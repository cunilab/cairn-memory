use crate::{AppState, PostgresAppState};
use cairn_core::{
    BackgroundJob, EMBED_BATCH, JOB_POLL_SECONDS, JobEnd, JobKind, Memory, MemoryEmbedding,
    embed_job_end, embedding_text, now_seconds,
};
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

const JOB_INTERVAL: Duration = Duration::from_secs(JOB_POLL_SECONDS);

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
                .embed(texts, Some(EMBED_BATCH))
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

fn spawn_interval<F, Fut>(tick: F)
where
    F: Fn() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(JOB_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            tick().await;
        }
    });
}

async fn run_file_tick(state: &AppState, model: Option<&LocalEmbeddings>) {
    let now = now_seconds();
    let embed_work = {
        let mut store = state.store.lock().await;
        let distill = match store.poll_jobs(JobKind::Distill, EMBED_BATCH, now) {
            Ok(claims) => claims,
            Err(error) => {
                eprintln!("file distill poll failed: {error}");
                return;
            }
        };
        for claim in distill {
            if let Err(error) = store.finish_claimed_distill(claim)
                && !matches!(
                    cairn_core::distill_job_end(&error.to_string()),
                    JobEnd::Succeeded
                )
            {
                eprintln!("file distill job failed: {error}");
            }
        }
        if model.is_none() {
            if let Err(error) = store.reconcile_embeddings(EMBED_BATCH, now_seconds()) {
                eprintln!("file embedding reconcile failed: {error}");
            }
            return;
        }
        let claims = match store.poll_jobs(JobKind::Embed, EMBED_BATCH, now_seconds()) {
            Ok(claims) => claims,
            Err(error) => {
                eprintln!("file embedding poll failed: {error}");
                return;
            }
        };
        match store.prepare_embed_work(claims, now_seconds()) {
            Ok(work) => work,
            Err(error) => {
                eprintln!("file embedding prepare failed: {error}");
                return;
            }
        }
    };
    let Some(model) = model else {
        return;
    };
    if embed_work.is_empty() {
        return;
    }
    let (claims, memories): (Vec<BackgroundJob>, Vec<Memory>) = embed_work.into_iter().unzip();
    match model.embed(memories).await {
        Ok(rows) => {
            let mut store = state.store.lock().await;
            for (claim, row) in claims.into_iter().zip(rows) {
                let claim_id = claim.claim_id.unwrap_or_default();
                let end = match store.put_embedding(row) {
                    Ok(_) => JobEnd::Succeeded,
                    Err(error) => embed_job_end(&error.to_string()),
                };
                if let Err(error) = store.settle_job(&claim.job_id, &claim_id, now_seconds(), end) {
                    eprintln!("file embedding settle failed: {error}");
                }
            }
        }
        Err(error) => {
            let end = embed_job_end(&error);
            let mut store = state.store.lock().await;
            for claim in claims {
                let claim_id = claim.claim_id.unwrap_or_default();
                if let Err(settle) =
                    store.settle_job(&claim.job_id, &claim_id, now_seconds(), end.clone())
                {
                    eprintln!("file embedding settle failed: {settle}");
                }
            }
        }
    }
}

pub(crate) fn spawn_file_jobs(state: &AppState) {
    let state = state.clone();
    let model = state.embeddings.clone();
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut store = state.store.lock().await;
            if let Err(error) = store.reconcile_embeddings(512, now_seconds()) {
                eprintln!("file embedding reconcile failed: {error}");
            }
        });
    }
    spawn_interval(move || {
        let state = state.clone();
        let model = model.clone();
        async move { run_file_tick(&state, model.as_ref()).await }
    });
}

async fn run_postgres_tick(state: &PostgresAppState, model: Option<&LocalEmbeddings>) {
    let now = now_seconds();
    let distill = match state
        .store
        .poll_jobs(JobKind::Distill, EMBED_BATCH, now)
        .await
    {
        Ok(claims) => claims,
        Err(error) => {
            eprintln!("PostgreSQL distill poll failed: {error}");
            return;
        }
    };
    for claim in distill {
        if let Err(error) = state.store.execute_claimed_distill(&claim).await
            && !matches!(
                cairn_core::distill_job_end(&error.to_string()),
                JobEnd::Succeeded
            )
        {
            eprintln!("PostgreSQL distill job failed: {error}");
        }
    }
    if model.is_none() {
        if let Err(error) = state
            .store
            .reconcile_embeddings(EMBED_BATCH, now_seconds())
            .await
        {
            eprintln!("PostgreSQL embedding reconcile failed: {error}");
        }
        return;
    }
    let claims = match state
        .store
        .poll_jobs(JobKind::Embed, EMBED_BATCH, now_seconds())
        .await
    {
        Ok(claims) => claims,
        Err(error) => {
            eprintln!("PostgreSQL embedding poll failed: {error}");
            return;
        }
    };
    if claims.is_empty() {
        return;
    }
    let ids = claims
        .iter()
        .map(|claim| claim.key.clone())
        .collect::<Vec<_>>();
    let needed = match state.store.memories_needing_embedding(&ids).await {
        Ok(rows) => rows,
        Err(error) => {
            eprintln!("PostgreSQL embedding load failed: {error}");
            return;
        }
    };
    let mut store_claims = Vec::new();
    let mut memories = Vec::new();
    for claim in claims {
        let claim_id = claim.claim_id.clone().unwrap_or_default();
        if let Some(memory) = needed.iter().find(|memory| memory.memory_id == claim.key) {
            memories.push(memory.clone());
            store_claims.push(claim);
        } else if let Err(error) = state
            .store
            .settle_job(&claim.job_id, &claim_id, now_seconds(), JobEnd::Succeeded)
            .await
        {
            eprintln!("PostgreSQL embedding settle failed: {error}");
        }
    }
    let Some(model) = model else {
        return;
    };
    if memories.is_empty() {
        return;
    }
    match model.embed(memories).await {
        Ok(rows) => {
            for (claim, row) in store_claims.into_iter().zip(rows) {
                let claim_id = claim.claim_id.unwrap_or_default();
                let end = match state.store.put_embedding(row).await {
                    Ok(_) => JobEnd::Succeeded,
                    Err(error) => embed_job_end(&error.to_string()),
                };
                if let Err(error) = state
                    .store
                    .settle_job(&claim.job_id, &claim_id, now_seconds(), end)
                    .await
                {
                    eprintln!("PostgreSQL embedding settle failed: {error}");
                }
            }
        }
        Err(error) => {
            let end = embed_job_end(&error);
            for claim in store_claims {
                let claim_id = claim.claim_id.unwrap_or_default();
                if let Err(settle) = state
                    .store
                    .settle_job(&claim.job_id, &claim_id, now_seconds(), end.clone())
                    .await
                {
                    eprintln!("PostgreSQL embedding settle failed: {settle}");
                }
            }
        }
    }
}

pub(crate) fn spawn_postgres_jobs(state: &PostgresAppState) {
    let state = state.clone();
    let model = state.embeddings.clone();
    {
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = state.store.reconcile_embeddings(512, now_seconds()).await {
                eprintln!("PostgreSQL embedding reconcile failed: {error}");
            }
        });
    }
    spawn_interval(move || {
        let state = state.clone();
        let model = model.clone();
        async move { run_postgres_tick(&state, model.as_ref()).await }
    });
}
