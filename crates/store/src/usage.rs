//! Retrieval usage: how often agents look memory up and how much text comes back. Counts only;
//! never what was asked.

use crate::memory::visible;
use crate::{Result, Store};
use sqlx::{PgConnection, Postgres, Transaction};

/// Rows older than this are dropped by the idle sweep.
pub const RETRIEVAL_RETENTION_DAYS: i64 = 120;
pub(crate) const SHOWN_RETENTION_DAYS: i64 = 7;
pub(crate) const SESSION_PUSH_LIMIT: usize = 8;
/// L2 automatic recall budget items (`source = 'brief'` deduplicates without consuming L2 slots).
pub(crate) const SESSION_RECALL_BUDGET: &str = "SELECT memory_id AS id FROM session_shown
      WHERE person_id = $1 AND session_id = $2 AND source <> 'brief'
     UNION ALL
     SELECT skill_id AS id FROM session_skill_shown WHERE person_id = $1 AND session_id = $2";

#[derive(Default)]
pub(crate) struct SessionShown {
    /// All memory and skill IDs already sent in this session (including brief), for deduplication.
    pub ids: Vec<String>,
    /// Cards charged against `SESSION_PUSH_LIMIT` (excludes `source = 'brief'`).
    pub recall_count: usize,
}

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

/// An explicit lookup after muting restores eligibility, as does any use of a push.
fn should_mute(pushed: i32, used: i32, revived: bool) -> bool {
    pushed >= 3 && used == 0 && !revived
}

#[cfg(test)]
mod tests;

pub(crate) fn unmuted(memory: &str, person: &str) -> String {
    format!("({memory}.pinned_at IS NOT NULL OR NOT EXISTS (SELECT 1 FROM push_usage pu
                          WHERE pu.person_id = {person} AND pu.memory_id = {memory}.memory_id AND pu.muted))")
}

async fn refresh_mutes(
    db: &mut PgConnection,
    person_id: &str,
    counts: Vec<(String, i32, i32, bool)>,
) -> Result<()> {
    let mute: Vec<_> = counts
        .into_iter()
        .filter_map(|(id, pushed, used, revived)| should_mute(pushed, used, revived).then_some(id))
        .collect();
    if !mute.is_empty() {
        sqlx::query(
            "UPDATE push_usage SET muted = true WHERE person_id = $1 AND memory_id = ANY($2)",
        )
        .bind(person_id)
        .bind(mute)
        .execute(db)
        .await?;
    }
    Ok(())
}

impl Store {
    /// An explicit search hit, get or helpful vote is a use of a push, muted or not. It is charged
    /// once to its session's ledger row; with no session, or no row for it, each call credits one
    /// use. It also restores a muted card.
    pub async fn record_push_use(
        &self,
        person_id: &str,
        session_id: Option<&str>,
        ids: &[String],
        revive: bool,
    ) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        let mut used = Vec::<String>::new();
        if let Some(session) = session_id {
            // An explicit use takes precedence over inferred use, with one credit per row.
            used = sqlx::query_scalar::<_, Option<String>>(&format!(
                "WITH old AS MATERIALIZED (
                    SELECT s.memory_id, s.used_at IS NULL AS first_use FROM session_shown s
                      JOIN memories m USING (memory_id)
                     WHERE s.person_id = $1 AND s.session_id = $2 AND s.memory_id = ANY($3)
                       AND (s.used_at IS NULL OR s.inferred_use) AND {}
                     ORDER BY s.memory_id FOR UPDATE OF s
                 )
                 UPDATE session_shown s SET used_at = now(), inferred_use = false
                   FROM old WHERE s.person_id = $1 AND s.session_id = $2 AND s.memory_id = old.memory_id
                 RETURNING CASE WHEN old.first_use THEN s.memory_id END",
                visible("m", "$1")
            ))
            .bind(person_id)
            .bind(session)
            .bind(ids)
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .flatten()
            .collect();
        }
        if revive || !used.is_empty() {
            sqlx::query(&format!(
                "UPDATE push_usage pu SET used = pu.used + (pu.memory_id = ANY($2) OR ($4 AND NOT EXISTS (
                            SELECT 1 FROM session_shown s
                             WHERE s.person_id = $1 AND s.session_id = $5 AND s.memory_id = pu.memory_id)))::int,
                        revived = pu.revived OR ($4 AND pu.muted), muted = false
                  FROM memories m
                 WHERE pu.person_id = $1 AND pu.memory_id = m.memory_id AND {}
                   AND (pu.memory_id = ANY($2) OR ($4 AND pu.memory_id = ANY($3)))",
                visible("m", "$1")
            ))
            .bind(person_id)
            .bind(used)
            .bind(ids)
            .bind(revive)
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

/// A late repeat (including after SessionEnd on a resumed host) revokes inferred use.
/// Keep explicit gets and votes, regardless of the outcome of the command.
pub(crate) async fn repeat_error(
    tx: &mut Transaction<'_, Postgres>,
    person_id: &str,
    session_id: &str,
    source: &str,
) -> Result<()> {
    let counts = sqlx::query_as(
        "WITH old AS MATERIALIZED (
             SELECT memory_id, inferred_use FROM session_shown
              WHERE person_id = $1 AND session_id = $2 AND source = $3 AND NOT error_repeated
              ORDER BY memory_id FOR UPDATE
         ), repeated AS (
             UPDATE session_shown s
                SET error_repeated = true, inferred_use = false,
                    used_at = CASE WHEN old.inferred_use THEN NULL ELSE s.used_at END
               FROM old WHERE s.person_id = $1 AND s.session_id = $2 AND s.memory_id = old.memory_id
                 RETURNING s.memory_id, old.inferred_use
         )
         UPDATE push_usage pu SET used = used - 1 FROM repeated r
          WHERE pu.person_id = $1 AND pu.memory_id = r.memory_id AND r.inferred_use
          RETURNING pu.memory_id, pushed, used, revived",
    )
    .bind(person_id)
    .bind(session_id)
    .bind(source)
    .fetch_all(&mut **tx)
    .await?;
    refresh_mutes(tx, person_id, counts).await
}

pub(crate) async fn settle_errors(
    tx: &mut Transaction<'_, Postgres>,
    session: Option<(&str, &str)>,
) -> Result<()> {
    let (person, id) = session.unzip();
    sqlx::query(
        "WITH settled AS (
            UPDATE session_shown
               SET used_at = now(), inferred_use = true
             WHERE used_at IS NULL AND NOT error_repeated AND source LIKE 'error:%'
               AND ((person_id = $1 AND session_id = $2)
                    OR ($1 IS NULL AND shown_at < now() - make_interval(days => $3)))
             RETURNING person_id, memory_id
         ), credits AS (
            SELECT person_id, memory_id, count(*)::int AS amount FROM settled
             GROUP BY person_id, memory_id
         )
         UPDATE push_usage pu SET used = pu.used + c.amount, muted = false
           FROM credits c WHERE pu.person_id = c.person_id AND pu.memory_id = c.memory_id",
    )
    .bind(person)
    .bind(id)
    .bind(SHOWN_RETENTION_DAYS as i32)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Serialize automatic pushes for a person, so simultaneous sessions cannot bypass muting.
pub(crate) async fn already_shown(
    tx: &mut Transaction<'_, Postgres>,
    person_id: &str,
    session_id: Option<&str>,
) -> Result<SessionShown> {
    let Some(session_id) = session_id else {
        return Ok(SessionShown::default());
    };
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtext(current_schema() || ':session_shown'), hashtext($1))",
    )
    .bind(person_id)
    .execute(&mut **tx)
    .await?;
    let rows: Vec<(String, bool)> = sqlx::query_as(
        "SELECT memory_id AS id, (source <> 'brief') AS counts_toward_budget
           FROM session_shown WHERE person_id = $1 AND session_id = $2
         UNION ALL
         SELECT skill_id AS id, true AS counts_toward_budget
           FROM session_skill_shown WHERE person_id = $1 AND session_id = $2",
    )
    .bind(person_id)
    .bind(session_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut ids = Vec::with_capacity(rows.len());
    let mut recall_count = 0usize;
    for (id, counts) in rows {
        ids.push(id);
        if counts {
            recall_count += 1;
        }
    }
    Ok(SessionShown { ids, recall_count })
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
            let counts = sqlx::query_as(
                "WITH shown AS (
                     INSERT INTO session_shown (person_id, session_id, memory_id, source)
                     SELECT $1, $2, id, $4 FROM unnest($3::text[]) AS id ORDER BY id
                     RETURNING memory_id
                 )
                 INSERT INTO push_usage (person_id, memory_id, pushed)
                 SELECT $1, memory_id, 1 FROM shown ORDER BY memory_id
                 ON CONFLICT (person_id, memory_id) DO UPDATE SET pushed = push_usage.pushed + 1
                 RETURNING memory_id, pushed, used, revived",
            )
            .bind(person_id)
            .bind(session_id)
            .bind(ids)
            .bind(source)
            .fetch_all(&mut *tx)
            .await?;
            refresh_mutes(&mut tx, person_id, counts).await?;
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
