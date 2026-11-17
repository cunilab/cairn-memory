use crate::{AppState, PostgresAppState};
use cairn_core::{Memory, MemoryEmbedding, embedding_text};
use fastembed::{
    AutoDevicePolicy, InitOptionsUserDefined, Pooling, TextEmbedding, TokenizerFiles,
    UserDefinedEmbeddingModel,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

const BATCH_SIZE: usize = 8;
const JOB_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct LocalEmbeddings(Arc<Mutex<TextEmbedding>>);

impl LocalEmbeddings {
    /// Verify every local artifact before loading; this path has no network-enabled loader.
    pub fn load(directory: &Path) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../../models/minilm.json"))?;
        let mut files = HashMap::new();
        for (name, hash) in manifest["files"]
            .as_object()
            .ok_or("invalid model manifest")?
        {
            let bytes = std::fs::read(directory.join(name))?;
            if Some(format!("{:x}", Sha256::digest(&bytes)).as_str()) != hash.as_str() {
                return Err("model artifact checksum mismatch".into());
            }
            files.insert(name.as_str(), bytes);
        }
        let mut take = |name| files.remove(name).ok_or("missing model artifact");
        let model = UserDefinedEmbeddingModel::new(
            take("model.onnx")?,
            TokenizerFiles {
                tokenizer_file: take("tokenizer.json")?,
                config_file: take("config.json")?,
                special_tokens_map_file: take("special_tokens_map.json")?,
                tokenizer_config_file: take("tokenizer_config.json")?,
            },
        )
        .with_pooling(Pooling::Mean);
        let model = TextEmbedding::try_new_from_user_defined(
            model,
            InitOptionsUserDefined::new()
                .with_max_length(256)
                .with_intra_threads(1)
                .with_auto_device(AutoDevicePolicy::PreferCPU),
        )?;
        Ok(Self(Arc::new(Mutex::new(model))))
    }

    pub async fn query(&self, query: &str) -> Option<Vec<f32>> {
        let model = self.0.clone();
        let query = query.chars().take(8192).collect::<String>();
        match tokio::task::spawn_blocking(move || {
            let Ok(mut model) = model.try_lock() else {
                return Ok(None);
            };
            model
                .embed(vec![query], Some(1))
                .map(|mut vectors| vectors.pop())
        })
        .await
        {
            Ok(Ok(vector)) => vector,
            _ => {
                eprintln!("semantic query unavailable; using lexical/entity/link recall");
                None
            }
        }
    }

    async fn embed(&self, memories: Vec<Memory>) -> Result<Vec<MemoryEmbedding>, String> {
        if memories.is_empty() {
            return Ok(Vec::new());
        }
        let model = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let texts = memories.iter().map(embedding_text).collect::<Vec<_>>();
            let mut model = model
                .lock()
                .map_err(|_| "embedding model lock poisoned".to_owned())?;
            let vectors = model
                .embed(texts, Some(BATCH_SIZE))
                .map_err(|_| "embedding inference failed".to_owned())?;
            if vectors.len() != memories.len() {
                return Err("embedding batch length mismatch".to_owned());
            }
            memories
                .iter()
                .zip(vectors)
                .map(|(m, v)| MemoryEmbedding::new(m, v).map_err(|e| e.to_string()))
                .collect()
        })
        .await
        .map_err(|_| "embedding worker failed".to_owned())?
    }
}

pub(crate) fn spawn_file_jobs(state: &AppState) {
    let Some(model) = state.embeddings.clone() else {
        return;
    };
    let state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(JOB_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let jobs = state.store.lock().await.embedding_jobs(BATCH_SIZE);
            match model.embed(jobs).await {
                Ok(rows) => {
                    let mut store = state.store.lock().await;
                    for row in rows {
                        if store.put_embedding(row).is_err() {
                            eprintln!("file embedding job persistence failed; will retry");
                            break;
                        }
                    }
                }
                Err(_) => eprintln!("file embedding job failed; will retry"),
            }
        }
    });
}

pub(crate) fn spawn_postgres_jobs(state: &PostgresAppState) {
    let Some(model) = state.embeddings.clone() else {
        return;
    };
    let state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(JOB_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let rows = match state.store.embedding_jobs(BATCH_SIZE).await {
                Ok(rows) => rows,
                Err(_) => {
                    eprintln!("PostgreSQL embedding scan failed; will retry");
                    continue;
                }
            };
            match model.embed(rows).await {
                Ok(rows) => {
                    for row in rows {
                        if state.store.put_embedding(row).await.is_err() {
                            eprintln!("PostgreSQL embedding persistence failed; will retry");
                            break;
                        }
                    }
                }
                Err(_) => eprintln!("PostgreSQL embedding job failed; will retry"),
            }
        }
    });
}
