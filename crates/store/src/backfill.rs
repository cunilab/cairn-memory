//! One-time backfill of the file and command entities that saves derive from evidence since
//! 0.1.12. Insert-only: it never updates or deletes a memory or an existing entity.

use crate::{Result, Store};
use cairn_core::capture::WorkContext;
use cairn_core::process::entities::{ENTITY_CAP, derive_with_evidence};
use sqlx::Row;

/// `server_state` key: the last memory id scanned, or `done`.
const EVIDENCE_KEY: &str = "backfill_evidence_entities";
/// Memories scanned per call; each call is one short transaction.
pub const EVIDENCE_BATCH: i64 = 200;

impl Store {
    /// Scans the next `batch` active memories that carry evidence and inserts their evidence
    /// entities, within each memory's 32-entity cap. Returns how many it scanned; 0 means finished.
    /// The cursor row lock serialises servers sharing a database, and a rerun inserts nothing new.
    pub async fn backfill_evidence_entities(&self, batch: i64) -> Result<usize> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO server_state (key, value) VALUES ($1, '') ON CONFLICT DO NOTHING")
            .bind(EVIDENCE_KEY)
            .execute(&mut *tx)
            .await?;
        let cursor: String =
            sqlx::query_scalar("SELECT value FROM server_state WHERE key = $1 FOR UPDATE")
                .bind(EVIDENCE_KEY)
                .fetch_one(&mut *tx)
                .await?;
        if cursor == "done" {
            return Ok(0);
        }
        let rows = sqlx::query(
            "SELECT m.memory_id, m.evidence, m.repo_worktree,
                    (SELECT count(*) FROM entities e WHERE e.memory_id = m.memory_id) AS have
               FROM memories m
              WHERE m.memory_id > $1 AND m.status = 'active' AND m.evidence <> '{}'
              ORDER BY m.memory_id LIMIT $2",
        )
        .bind(&cursor)
        .bind(batch)
        .fetch_all(&mut *tx)
        .await?;
        let mut last = cursor;
        for row in &rows {
            let id: String = row.try_get("memory_id")?;
            let evidence: Vec<String> = row.try_get("evidence")?;
            let have: i64 = row.try_get("have")?;
            let work = WorkContext {
                worktree: row.try_get("repo_worktree")?,
                ..WorkContext::default()
            };
            let room = ENTITY_CAP.saturating_sub(usize::try_from(have).unwrap_or(ENTITY_CAP));
            let (kinds, names): (Vec<&str>, Vec<String>) =
                derive_with_evidence(&[], &evidence, &work)
                    .into_iter()
                    .take(room)
                    .map(|e| (e.kind.as_str(), e.name))
                    .unzip();
            if !kinds.is_empty() {
                sqlx::query(
                    "INSERT INTO entities (memory_id, kind, name)
                     SELECT $1, k, n FROM unnest($2::text[], $3::text[]) AS t(k, n)
                     ON CONFLICT DO NOTHING",
                )
                .bind(&id)
                .bind(&kinds)
                .bind(&names)
                .execute(&mut *tx)
                .await?;
            }
            last = id;
        }
        let finished = rows.len() < usize::try_from(batch).unwrap_or(usize::MAX);
        sqlx::query("UPDATE server_state SET value = $2 WHERE key = $1")
            .bind(EVIDENCE_KEY)
            .bind(if finished { "done" } else { last.as_str() })
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(rows.len())
    }
}
