//! One person's opt-in search trace. Rows last 7 days and never hold memory text.

use crate::{Store, StoreError};
use cairn_core::capture::redact_sensitive_content;
use cairn_core::id::new_id;
use serde_json::{Value, json};
use sqlx::Row;

const QUERY_CHARS: usize = 160;
const LIST_LIMIT: i64 = 50;

pub struct TraceHit {
    pub id: String,
    pub rank: i32,
    pub channel: String,
}

impl Store {
    pub async fn retrieval_trace_enabled(&self, person_id: &str) -> Result<bool, StoreError> {
        let enabled: bool =
            sqlx::query_scalar("SELECT retrieval_trace FROM accounts WHERE person_id = $1")
                .bind(person_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(enabled)
    }

    pub async fn set_retrieval_trace(
        &self,
        person_id: &str,
        enabled: bool,
    ) -> Result<(), StoreError> {
        sqlx::query("UPDATE accounts SET retrieval_trace = $2 WHERE person_id = $1")
            .bind(person_id)
            .bind(enabled)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Best-effort write. Does nothing when the person has not turned the trace on.
    /// Deletes expired rows. Stores no memory title, summary or detail.
    pub async fn record_retrieval_trace(
        &self,
        person_id: &str,
        query: &str,
        hits: &[TraceHit],
    ) -> Result<(), StoreError> {
        self.expire_retrieval_traces().await?;
        if !self.retrieval_trace_enabled(person_id).await? {
            return Ok(());
        }
        let mut query_redacted: String = redact_sensitive_content(query)
            .chars()
            .take(QUERY_CHARS)
            .collect();
        if query_redacted.trim().is_empty() {
            query_redacted = "(empty)".to_owned();
        }
        let results = hits
            .iter()
            .map(|hit| {
                json!({
                    "id": hit.id,
                    "rank": hit.rank,
                    "channel": channel_only(&hit.channel),
                    "opened": false,
                    "voted": false,
                })
            })
            .collect::<Vec<_>>();
        sqlx::query(
            "INSERT INTO retrieval_traces (trace_id, person_id, expires_at, query_redacted, results)
             VALUES ($1, $2, now() + interval '7 days', $3, $4)",
        )
        .bind(new_id("trc"))
        .bind(person_id)
        .bind(query_redacted)
        .bind(Value::Array(results))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The caller's own unexpired rows, newest first.
    pub async fn list_retrieval_trace(&self, person_id: &str) -> Result<Value, StoreError> {
        self.expire_retrieval_traces().await?;
        let enabled = self.retrieval_trace_enabled(person_id).await?;
        let rows = sqlx::query(
            "SELECT trace_id, query_redacted, results,
                    EXTRACT(EPOCH FROM created_at)::bigint AS created_at
               FROM retrieval_traces
              WHERE person_id = $1 AND expires_at > now()
              ORDER BY created_at DESC, trace_id DESC
              LIMIT $2",
        )
        .bind(person_id)
        .bind(LIST_LIMIT)
        .fetch_all(&self.pool)
        .await?;
        let traces = rows
            .iter()
            .map(|row| {
                Ok(json!({
                    "id": row.try_get::<String, _>("trace_id")?,
                    "query": row.try_get::<String, _>("query_redacted")?,
                    "results": row.try_get::<Value, _>("results")?,
                    "created_at": row.try_get::<i64, _>("created_at")?,
                }))
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        Ok(json!({ "enabled": enabled, "traces": traces }))
    }

    pub async fn mark_retrieval_trace(
        &self,
        person_id: &str,
        memory_id: &str,
        voted: bool,
    ) -> Result<(), StoreError> {
        let rows = sqlx::query(
            "SELECT trace_id, results FROM retrieval_traces
              WHERE person_id = $1 AND expires_at > now()
              ORDER BY created_at DESC LIMIT 20",
        )
        .bind(person_id)
        .fetch_all(&self.pool)
        .await?;
        for row in rows {
            let id: String = row.try_get("trace_id")?;
            // The column is one jsonb array, not jsonb[].
            let Value::Array(mut results) = row.try_get::<Value, _>("results")? else {
                continue;
            };
            let mut changed = false;
            for item in &mut results {
                if item.get("id").and_then(Value::as_str) == Some(memory_id) {
                    let key = if voted { "voted" } else { "opened" };
                    if item.get(key) != Some(&Value::Bool(true)) {
                        item[key] = Value::Bool(true);
                        changed = true;
                    }
                }
            }
            if changed {
                sqlx::query("UPDATE retrieval_traces SET results = $2 WHERE trace_id = $1 AND person_id = $3")
                    .bind(&id)
                    .bind(Value::Array(results))
                    .bind(person_id)
                    .execute(&self.pool)
                    .await?;
            }
        }
        Ok(())
    }

    async fn expire_retrieval_traces(&self) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM retrieval_traces WHERE expires_at <= now()")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

fn channel_only(why: &str) -> String {
    why.split_whitespace()
        .next()
        .filter(|text| !text.is_empty())
        .unwrap_or("linked")
        .to_owned()
}
