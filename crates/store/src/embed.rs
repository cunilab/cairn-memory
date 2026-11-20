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
              WHERE m.status <> 'forgotten' AND (m.expires_at IS NULL OR m.expires_at > now())
                AND NOT EXISTS (SELECT 1 FROM memory_embeddings e WHERE e.memory_id = m.memory_id)
              ORDER BY m.created_at, m.memory_id LIMIT $1"
        ))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        self.embed_items_from(&rows).await
    }

    /// The same items for specific memories (the ids of claimed `embed` jobs). A forgotten or
    /// expired memory has no item.
    pub async fn embed_items(&self, ids: &[String]) -> Result<Vec<EmbedItem>> {
        let rows = sqlx::query(&format!(
            "{MEMORY_SELECT}
              WHERE m.memory_id = ANY($1) AND m.status <> 'forgotten'
                AND (m.expires_at IS NULL OR m.expires_at > now())"
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
            "INSERT INTO memory_embeddings (memory_id, model, source_hash, embedding)
             SELECT memory_id, $2, $3, $4::vector FROM memories WHERE memory_id = $1 AND status <> 'forgotten'
             ON CONFLICT (memory_id) DO UPDATE
                SET model = EXCLUDED.model, source_hash = EXCLUDED.source_hash,
                    embedding = EXCLUDED.embedding, created_at = now()
              WHERE memory_embeddings.source_hash IS DISTINCT FROM EXCLUDED.source_hash
                 OR memory_embeddings.model IS DISTINCT FROM EXCLUDED.model",
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
        Ok(
            sqlx::query_scalar("SELECT source_hash FROM memory_embeddings WHERE memory_id = $1")
                .bind(memory_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }
}
