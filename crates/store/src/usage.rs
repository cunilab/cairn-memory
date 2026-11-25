//! Retrieval usage: how often agents look memory up and how much text comes back. Counts only;
//! never what was asked.

use crate::memory::visible;
use crate::{Result, Store};

/// Rows older than this are dropped by the idle sweep.
pub const RETRIEVAL_RETENTION_DAYS: i64 = 120;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup {
    Search,
    Get,
    Brief,
    Topic,
}

impl Lookup {
    fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Get => "get",
            Self::Brief => "brief",
            Self::Topic => "topic",
        }
    }
}

impl Store {
    /// One lookup: `bytes` of response (tokens are estimated as bytes / 4) and whether it found
    /// anything. Kept in 15-minute buckets so a reader can group by any local day.
    /// Hook replies count the memory ids rendered into their context in the same database write.
    pub async fn record_retrieval(
        &self,
        person_id: &str,
        lookup: Lookup,
        bytes: usize,
        hit: bool,
        shown: &[String],
    ) -> Result<()> {
        let scope = visible("m", "$1");
        sqlx::query(&format!(
            "WITH shown AS (
                INSERT INTO usage (memory_id, shown, last_shown_at)
                SELECT m.memory_id, 1, now() FROM memories m
                WHERE m.memory_id = ANY($5) AND {scope}
                ORDER BY m.memory_id
                ON CONFLICT (memory_id) DO UPDATE
                    SET shown = usage.shown + 1,
                        last_shown_at = greatest(usage.last_shown_at, EXCLUDED.last_shown_at)
             )
             INSERT INTO retrieval_log (person_id, bucket, kind, calls, bytes, hits)
             VALUES ($1, to_timestamp(floor(extract(epoch FROM now()) / 900) * 900), $2, 1, $3, $4)
             ON CONFLICT (person_id, bucket, kind) DO UPDATE
                SET calls = retrieval_log.calls + 1, bytes = retrieval_log.bytes + EXCLUDED.bytes,
                    hits = retrieval_log.hits + EXCLUDED.hits",
        ))
        .bind(person_id)
        .bind(lookup.as_str())
        .bind(bytes as i64)
        .bind(i32::from(hit))
        .bind(shown)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
