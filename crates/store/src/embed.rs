use crate::jobs::enqueue_tx;
use crate::memory::{MEMORY_SELECT, memory_from_row};
use crate::{Result, Store, StoreError};
use cairn_core::memory::{Entity, EntityKind};
use cairn_core::process::embed::{embedding_text, model_label, source_hash, validate_vector};
use cairn_core::process::jobs::JobKind;
use sqlx::Row;
use std::collections::HashMap;

/// A memory that still needs a vector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbedItem {
    pub memory_id: String,
    pub text: String,
    pub source_hash: String,
}

/// `[v1,v2,..]`, the text form pgvector reads.
pub(crate) fn vector_text(vector: &[f32]) -> String {
    let items: Vec<String> = vector.iter().map(f32::to_string).collect();
    format!("[{}]", items.join(","))
}

impl Store {
    /// Live memories without a vector, oldest first. This is the internal worker input.
    pub async fn pending_embeddings(&self, limit: i64) -> Result<Vec<EmbedItem>> {
        let rows = sqlx::query(&format!(
            "{MEMORY_SELECT}
              WHERE m.status <> 'forgotten'
                AND m.embedding IS NULL
              ORDER BY m.created_at, m.memory_id LIMIT $1"
        ))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        self.embed_items_from(&rows).await
    }

    /// Queues an embed job for every live memory that has no vector and no pending or running
    /// job, `batch` at a time until none is left. Returns how many it queued.
    pub async fn enqueue_missing_embeddings(&self, batch: i64) -> Result<usize> {
        let mut queued = 0;
        loop {
            let ids: Vec<String> = sqlx::query_scalar(
                "SELECT m.memory_id FROM memories m
                  WHERE m.status <> 'forgotten'
                    AND m.embedding IS NULL
                    AND NOT EXISTS (SELECT 1 FROM jobs j WHERE j.kind = 'embed' AND j.job_key = m.memory_id
                                       AND j.status IN ('pending', 'running'))
                  ORDER BY m.created_at, m.memory_id LIMIT $1",
            )
            .bind(batch)
            .fetch_all(&self.pool)
            .await?;
            let mut added = 0;
            for id in &ids {
                added += usize::from(self.enqueue_job(JobKind::Embed, id).await?);
            }
            if added == 0 {
                return Ok(queued);
            }
            queued += added;
        }
    }

    /// The same items for specific memories (the ids of claimed `embed` jobs). A forgotten memory
    /// has no item.
    pub async fn embed_items(&self, ids: &[String]) -> Result<Vec<EmbedItem>> {
        let rows = sqlx::query(&format!(
            "{MEMORY_SELECT}
              WHERE m.memory_id = ANY($1) AND m.status <> 'forgotten'"
        ))
        .bind(ids)
        .fetch_all(&self.pool)
        .await?;
        self.embed_items_from(&rows).await
    }

    async fn embed_items_from(&self, rows: &[sqlx::postgres::PgRow]) -> Result<Vec<EmbedItem>> {
        let mut memories = rows
            .iter()
            .map(memory_from_row)
            .collect::<Result<Vec<_>>>()?;
        let ids: Vec<String> = memories.iter().map(|m| m.id.clone()).collect();
        let mut entities: HashMap<String, Vec<Entity>> = HashMap::new();
        for row in sqlx::query("SELECT memory_id, kind, name FROM entities WHERE memory_id = ANY($1) ORDER BY kind, name")
            .bind(&ids)
            .fetch_all(&self.pool)
            .await?
        {
            let kind: EntityKind = row.try_get::<String, _>("kind")?.parse().map_err(StoreError::Validation)?;
            entities.entry(row.try_get("memory_id")?).or_default().push(Entity { kind, name: row.try_get("name")? });
        }
        Ok(memories
            .iter_mut()
            .map(|memory| {
                memory.entities = entities.remove(&memory.id).unwrap_or_default();
                let text = embedding_text(memory);
                EmbedItem {
                    memory_id: memory.id.clone(),
                    source_hash: source_hash(&text),
                    text,
                }
            })
            .collect())
    }

    /// Stores a vector for a live memory. The same `source_hash` and model is a no-op (false).
    /// A written vector queues a `consolidate` job for the memory.
    pub async fn upsert_embedding(
        &self,
        memory_id: &str,
        source_hash: &str,
        vector: &[f32],
    ) -> Result<bool> {
        validate_vector(vector)?;
        let mut tx = self.pool.begin().await?;
        let done = sqlx::query(
            "UPDATE memories SET embedding = $4::vector, embedding_model = $2, embedding_source = $3
              WHERE memory_id = $1 AND status <> 'forgotten'
                AND (embedding IS NULL OR embedding_source IS DISTINCT FROM $3
                     OR embedding_model IS DISTINCT FROM $2)",
        )
        .bind(memory_id)
        .bind(model_label())
        .bind(source_hash)
        .bind(vector_text(vector))
        .execute(&mut *tx)
        .await?;
        let written = done.rows_affected() == 1;
        if written {
            enqueue_tx(&mut tx, JobKind::Consolidate, memory_id).await?;
        }
        tx.commit().await?;
        Ok(written)
    }

    pub async fn embedding_source_hash(&self, memory_id: &str) -> Result<Option<String>> {
        Ok(sqlx::query_scalar::<_, Option<String>>(
            "SELECT embedding_source FROM memories WHERE memory_id = $1",
        )
        .bind(memory_id)
        .fetch_optional(&self.pool)
        .await?
        .flatten())
    }
}
