//! Retrieval usage: how often agents look memory up and how much text comes back. Counts only;
//! never what was asked.

use crate::memory::visible;
use crate::{Result, Store};
use sqlx::{Postgres, Transaction};

/// Rows older than this are dropped by the idle sweep.
pub const RETRIEVAL_RETENTION_DAYS: i64 = 120;
pub(crate) const SHOWN_RETENTION_DAYS: i64 = 7;
pub(crate) const SESSION_PUSH_LIMIT: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup {
    Search,
    Get,
    Brief,
    Topic,
}

impl Lookup {
    pub(crate) fn as_str(self) -> &'static str {
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
    pub async fn record_retrieval(
        &self,
        person_id: &str,
        lookup: Lookup,
        bytes: usize,
        hit: bool,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO retrieval_log (person_id, bucket, kind, calls, bytes, hits)
             VALUES ($1, to_timestamp(floor(extract(epoch FROM now()) / 900) * 900), $2, 1, $3, $4)
             ON CONFLICT (person_id, bucket, kind) DO UPDATE
                SET calls = retrieval_log.calls + 1, bytes = retrieval_log.bytes + EXCLUDED.bytes,
                    hits = retrieval_log.hits + EXCLUDED.hits",
        )
        .bind(person_id)
        .bind(lookup.as_str())
        .bind(bytes as i64)
        .bind(i32::from(hit))
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// Serialize selection and recording for one person’s session, including concurrent hook calls.
pub(crate) async fn already_shown(
    tx: &mut Transaction<'_, Postgres>,
    person_id: &str,
    session_id: Option<&str>,
) -> Result<Vec<String>> {
    let Some(session_id) = session_id else {
        return Ok(Vec::new());
    };
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtext(current_schema() || ':session_shown:' || $1), hashtext($2))",
    )
    .bind(person_id)
    .bind(session_id)
    .execute(&mut **tx)
    .await?;
    Ok(sqlx::query_scalar(
        "SELECT memory_id FROM session_shown WHERE person_id = $1 AND session_id = $2",
    )
    .bind(person_id)
    .bind(session_id)
    .fetch_all(&mut **tx)
    .await?)
}

/// Commit both the reservation and its usage counter for exactly the cards that fit.
pub(crate) async fn finish_push(
    mut tx: Transaction<'_, Postgres>,
    person_id: &str,
    session_id: Option<&str>,
    source: &str,
    ids: &[String],
) -> Result<()> {
    if !ids.is_empty() {
        if let Some(session_id) = session_id {
            sqlx::query(
                "INSERT INTO session_shown (person_id, session_id, memory_id, source)
                 SELECT $1, $2, id, $4 FROM unnest($3::text[]) AS id ORDER BY id",
            )
            .bind(person_id)
            .bind(session_id)
            .bind(ids)
            .bind(source)
            .execute(&mut *tx)
            .await?;
        }
        let scope = visible("m", "$1");
        sqlx::query(&format!(
            "INSERT INTO usage (memory_id, shown, last_shown_at)
             SELECT m.memory_id, 1, now() FROM memories m
              WHERE m.memory_id = ANY($2) AND {scope}
              ORDER BY m.memory_id
             ON CONFLICT (memory_id) DO UPDATE
                 SET shown = usage.shown + 1,
                     last_shown_at = greatest(usage.last_shown_at, EXCLUDED.last_shown_at)",
        ))
        .bind(person_id)
        .bind(ids)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
