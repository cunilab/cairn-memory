use crate::embed::vector_text;
use crate::memory::{MEMORY_SELECT, memory_from_row, visible};
use crate::{Result, Store};
use cairn_core::memory::Memory;
use cairn_core::process::embed::validate_vector;
use sqlx::Row;

/// Candidate channels for hybrid search. Each returns `(memory_id, score)` for memories the
/// person may see; fusion is `cairn_core::retrieve::hybrid::fuse`.
impl Store {
    /// Every memory the person can see, with entities and links. Step 5 replaces this for search.
    pub async fn visible_memories(&self, person_id: &str) -> Result<Vec<Memory>> {
        let rows = sqlx::query(&format!(
            "{MEMORY_SELECT} WHERE {} ORDER BY m.updated_at DESC, m.memory_id DESC",
            visible("m", "$1")
        ))
        .bind(person_id)
        .fetch_all(&self.pool)
        .await?;
        let mut memories = rows
            .iter()
            .map(memory_from_row)
            .collect::<Result<Vec<_>>>()?;
        self.attach(person_id, &mut memories).await?;
        Ok(memories)
    }

    /// Full-text channel: `websearch_to_tsquery('english', q)` over `memories_search_idx`.
    pub async fn lexical_candidates(
        &self,
        person_id: &str,
        query: &str,
        limit: i64,
    ) -> Result<Vec<(String, f32)>> {
        scored(
            sqlx::query(&format!(
                "SELECT m.memory_id, ts_rank_cd(m.search_tsv, q.query)::float4 AS score
                   FROM memories m, websearch_to_tsquery('english', $2) AS q(query)
                  WHERE m.search_tsv @@ q.query AND {}
                  ORDER BY score DESC, m.memory_id LIMIT $3",
                visible("m", "$1")
            ))
            .bind(person_id)
            .bind(query)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?,
        )
    }

    /// Entity channel: score is the number of matching entity names, via `entities_lookup_idx`.
    pub async fn entity_candidates(
        &self,
        person_id: &str,
        names: &[String],
        limit: i64,
    ) -> Result<Vec<(String, f32)>> {
        scored(
            sqlx::query(&format!(
                "SELECT e.memory_id, count(*)::float4 AS score
                   FROM entities e JOIN memories m ON m.memory_id = e.memory_id
                  WHERE e.name = ANY($2) AND {}
                  GROUP BY e.memory_id ORDER BY score DESC, e.memory_id LIMIT $3",
                visible("m", "$1")
            ))
            .bind(person_id)
            .bind(names)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?,
        )
    }

    /// Semantic channel: the nearest vectors by cosine through `memory_embeddings_hnsw`, then the
    /// visibility filter. Score is cosine similarity.
    pub async fn vector_candidates(
        &self,
        person_id: &str,
        vector: &[f32],
        limit: i64,
    ) -> Result<Vec<(String, f32)>> {
        validate_vector(vector)?;
        // hnsw.ef_search defaults to 40 and caps the rows an index scan returns, so a larger
        // LIMIT would silently come back short.
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT set_config('hnsw.ef_search', $1, true)")
            .bind(limit.clamp(40, 1000).to_string())
            .execute(&mut *tx)
            .await?;
        let rows = sqlx::query(&format!(
            "SELECT c.memory_id, (1 - c.dist)::float4 AS score
               FROM (SELECT memory_id, embedding <=> $2::vector AS dist
                       FROM memory_embeddings ORDER BY embedding <=> $2::vector LIMIT $3) c
               JOIN memories m ON m.memory_id = c.memory_id
              WHERE {} ORDER BY c.dist, c.memory_id",
            visible("m", "$1")
        ))
        .bind(person_id)
        .bind(vector_text(vector))
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        scored(rows)
    }
}

fn scored(rows: Vec<sqlx::postgres::PgRow>) -> Result<Vec<(String, f32)>> {
    rows.iter()
        .map(|r| Ok((r.try_get("memory_id")?, r.try_get("score")?)))
        .collect()
}
