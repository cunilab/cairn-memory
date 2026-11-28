//! Background work: distill sessions into notes and a handoff, embed memories, link them by shared
//! entities, consolidate near-duplicates, and sweep idle sessions and expired rows. One tick drains what is due.

use crate::AppState;
use cairn_core::process::embed::EMBED_BATCH;
use cairn_core::process::jobs::{
    JOB_POLL_SECONDS, JobEnd, JobKind, embed_job_end, is_empty_session, parse_distill_key,
};
use cairn_store::StoreError;
use cairn_store::jobs::Job;
use fastembed::{
    AutoDevicePolicy, InitOptionsUserDefined, Pooling, TextEmbedding, TokenizerFiles,
    UserDefinedEmbeddingModel,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often idle sessions and expired rows are swept.
const SWEEP_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Memories queued per round at start, for rows saved while no model was loaded. Rounds repeat
/// until none is left.
const RECONCILE_BATCH: i64 = 512;

#[derive(Clone)]
pub struct LocalEmbeddings(Arc<Mutex<TextEmbedding>>);

impl LocalEmbeddings {
    /// Verifies every local artifact against the manifest before loading; no network loader.
    pub fn load(directory: &Path) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../assets/minilm/minilm.json"))?;
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

    /// One query vector. Waits for the model lock (an embedding job holds it for one batch), so a
    /// search never silently drops its semantic channel; `None` only on a poisoned lock or an
    /// inference error.
    pub async fn query(&self, query: &str) -> Option<Vec<f32>> {
        let model = self.0.clone();
        let query = query.chars().take(8192).collect::<String>();
        match tokio::task::spawn_blocking(move || {
            let mut model = model.lock().ok()?;
            model.embed(vec![query], Some(1)).ok()?.pop()
        })
        .await
        {
            Ok(Some(vector)) => Some(vector),
            _ => {
                eprintln!("semantic query unavailable; using lexical/entity/link recall");
                None
            }
        }
    }

    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
        let model = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut model = model
                .lock()
                .map_err(|_| "embedding model lock poisoned".to_owned())?;
            let vectors = model
                .embed(texts.clone(), Some(EMBED_BATCH))
                .map_err(|_| "embedding inference failed".to_owned())?;
            if vectors.len() != texts.len() {
                return Err("embedding batch length mismatch".to_owned());
            }
            Ok(vectors)
        })
        .await
        .map_err(|_| "embedding worker failed".to_owned())?
    }
}

/// Starts the job loop and the sweep, after queueing memories that still lack a vector. The first
/// start of a new server version retries every quarantined job once: the upgrade may fix it.
pub fn spawn(state: &AppState) {
    let upgrade = state.clone();
    tokio::spawn(async move {
        match upgrade
            .store
            .retry_quarantined_after_upgrade(crate::VERSION)
            .await
        {
            Ok(0) => {}
            Ok(n) => println!(
                "retrying {n} quarantined job(s) after the upgrade to {}",
                crate::VERSION
            ),
            Err(error) => eprintln!("quarantine retry after upgrade failed: {error}"),
        }
    });
    let backfill = state.clone();
    tokio::spawn(async move {
        // One short batch at a time, so a large database never holds startup or hooks.
        loop {
            match backfill
                .store
                .backfill_evidence_entities(cairn_store::backfill::EVIDENCE_BATCH)
                .await
            {
                Ok(0) => break,
                Ok(_) => tokio::time::sleep(Duration::from_millis(200)).await,
                Err(error) => {
                    eprintln!(
                        "evidence entity backfill failed; it resumes on the next start: {error}"
                    );
                    break;
                }
            }
        }
    });
    let reconcile = state.clone();
    tokio::spawn(async move {
        if reconcile.embeddings.is_none() {
            return;
        }
        if let Err(error) = reconcile
            .store
            .enqueue_missing_embeddings(RECONCILE_BATCH)
            .await
        {
            eprintln!("embedding reconcile failed: {error}");
        }
    });
    let jobs = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(JOB_POLL_SECONDS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            tick(&jobs).await;
        }
    });
    let sweeper = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SWEEP_INTERVAL);
        interval.tick().await; // a fresh start has nothing idle to close
        loop {
            interval.tick().await;
            if let Err(error) = sweeper.store.sweep().await {
                eprintln!("sweep failed: {error}");
            }
            match sweeper.store.fixes_due().await {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) => {
                    eprintln!("fix schedule check failed: {error}");
                    continue;
                }
            }
            let phrases = match sweeper.store.fixes_alias_queries().await {
                Ok(phrases) => phrases,
                Err(error) => {
                    eprintln!("fix query poll failed: {error}");
                    continue;
                }
            };
            let mut vectors = Vec::new();
            let mut failed = false;
            if let Some(model) = &sweeper.embeddings {
                for phrase in phrases {
                    let Some(vector) = model.query(&phrase).await else {
                        eprintln!("fix query embedding failed");
                        failed = true;
                        break;
                    };
                    vectors.push((phrase, vector));
                }
            }
            if failed {
                continue;
            }
            if let Err(error) = sweeper
                .store
                .fixes_nightly_with_queries(sweeper.embeddings.is_some(), crate::VERSION, &vectors)
                .await
            {
                eprintln!("fix detector pass failed: {error}");
            }
        }
    });
}

/// Runs every due job once: distill, then embed, then link, then consolidate.
pub async fn tick(state: &AppState) {
    distill(state).await;
    embed(state).await;
    link(state).await;
    consolidate(state).await;
}

async fn claim(state: &AppState, kind: JobKind) -> Vec<Job> {
    state
        .store
        .claim_jobs(kind, EMBED_BATCH as i64)
        .await
        .unwrap_or_else(|error| {
            eprintln!("{} poll failed: {error}", kind.as_str());
            Vec::new()
        })
}

/// A failed job's error goes into the owner's timeline, so SQL detail stays in the server log.
fn job_error(error: &StoreError) -> String {
    match error {
        StoreError::Database(_) => {
            eprintln!("job failed: {error}");
            "database error".into()
        }
        other => other.to_string(),
    }
}

async fn settle(state: &AppState, job: &Job, end: JobEnd) {
    let claim_id = job.claim_id.as_deref().unwrap_or_default();
    if let Err(error) = state.store.settle_job(&job.job_id, claim_id, end).await {
        eprintln!("settle failed: {error}");
    }
}

async fn distill(state: &AppState) {
    for job in claim(state, JobKind::Distill).await {
        let end = match parse_distill_key(&job.key) {
            None => JobEnd::Quarantine("invalid distill key".into()),
            Some((person, session)) => match state.store.distill_session(&person, &session).await {
                Ok(_) => JobEnd::Succeeded,
                Err(error) if is_empty_session(&error.to_string()) => JobEnd::Succeeded,
                Err(error) => JobEnd::Retry(job_error(&error)),
            },
        };
        settle(state, &job, end).await;
    }
}

async fn embed(state: &AppState) {
    let jobs = claim(state, JobKind::Embed).await;
    if jobs.is_empty() {
        return;
    }
    let Some(model) = &state.embeddings else {
        // Nothing to embed with; the startup reconcile queues these again once a model loads.
        for job in &jobs {
            settle(state, job, JobEnd::Succeeded).await;
        }
        return;
    };
    let ids: Vec<String> = jobs.iter().map(|j| j.key.clone()).collect();
    let items = match state.store.embed_items(&ids).await {
        Ok(items) => items,
        Err(error) => {
            for job in &jobs {
                settle(state, job, JobEnd::Retry(job_error(&error))).await;
            }
            return;
        }
    };
    let texts: Vec<String> = items.iter().map(|i| i.text.clone()).collect();
    let vectors = if texts.is_empty() {
        Ok(Vec::new())
    } else {
        model.embed(texts).await
    };
    let mut results: HashMap<&str, JobEnd> = HashMap::new();
    match &vectors {
        Ok(vectors) => {
            for (item, vector) in items.iter().zip(vectors) {
                let end = match state
                    .store
                    .upsert_embedding(&item.memory_id, &item.source_hash, vector)
                    .await
                {
                    Ok(_) => JobEnd::Succeeded,
                    Err(error) => embed_job_end(&error.to_string()),
                };
                results.insert(&item.memory_id, end);
            }
        }
        Err(error) => {
            for item in &items {
                results.insert(&item.memory_id, embed_job_end(error));
            }
        }
    }
    for job in &jobs {
        // A memory with no item was forgotten or expired: nothing left to embed.
        let end = results
            .remove(job.key.as_str())
            .unwrap_or(JobEnd::Succeeded);
        settle(state, job, end).await;
    }
}

async fn link(state: &AppState) {
    for job in claim(state, JobKind::Link).await {
        let end = match state.store.link_memory(&job.key).await {
            Ok(_) => JobEnd::Succeeded,
            Err(error) => JobEnd::Retry(job_error(&error)),
        };
        settle(state, &job, end).await;
    }
}

async fn consolidate(state: &AppState) {
    for job in claim(state, JobKind::Consolidate).await {
        let end = match state.store.consolidate_memory(&job.key).await {
            Ok(_) => JobEnd::Succeeded,
            Err(error) => JobEnd::Retry(job_error(&error)),
        };
        settle(state, &job, end).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_database_error_never_reaches_the_timeline() {
        let secret = StoreError::Database(sqlx::Error::Protocol(
            "insert into memories (secret_column)".into(),
        ));
        assert_eq!(job_error(&secret), "database error");
        assert_eq!(
            job_error(&StoreError::NotFound("memory".into())),
            StoreError::NotFound("memory".into()).to_string()
        );
    }
}
