use crate::jobs::enqueue_tx;
use crate::memory::insert_memory;
use crate::{Result, Store, StoreError};
use cairn_core::capture::session::IDLE_SESSION_SECONDS;
use cairn_core::capture::{
    CaptureResult, Observation, ObservationInput, WorkContext, normalize_observation,
};
use cairn_core::id::now_seconds;
use cairn_core::memory::kind::EXPIRED_GRACE_SECONDS;
use cairn_core::process::distill::{distill, handoff_id};
use cairn_core::process::jobs::{JOB_DONE_RETENTION_SECONDS, JobKind, distill_key};
use cairn_core::process::{Origin, prepare_save};
use sqlx::Row;

/// What `distill_session` wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DistillReport {
    pub notes: usize,
    pub handoff: bool,
}

/// What one sweep did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub sessions_closed: u64,
    pub memories_deleted: u64,
    pub observations_deleted: u64,
    pub jobs_deleted: u64,
}

impl Store {
    /// Opens or refreshes a session row and returns nothing; captures also open sessions.
    pub async fn start_session(
        &self,
        person_id: &str,
        session_id: &str,
        work: &WorkContext,
    ) -> Result<()> {
        if session_id.trim().is_empty() {
            return Err(StoreError::Validation(
                "session_id must not be empty".into(),
            ));
        }
        let project_id = self.context_project(person_id, work).await?;
        upsert_session(
            &self.pool,
            person_id,
            session_id,
            project_id.as_deref(),
            work,
            false,
        )
        .await
    }

    /// Stores one redacted observation. The same event id with the same content is a duplicate;
    /// with other content it is a 409.
    pub async fn capture(&self, person_id: &str, input: ObservationInput) -> Result<CaptureResult> {
        let mut observation = normalize_observation(&input)?;
        // A bare project id (the CLI sends only that) still inherits the session's git context.
        let bare = WorkContext {
            project_id: None,
            ..observation.context.clone()
        } == WorkContext::default();
        if bare
            && let Some(inherited) = self
                .session_context(person_id, &observation.session_id)
                .await?
        {
            observation.context = WorkContext {
                project_id: observation.context.project_id.clone(),
                ..inherited
            };
        }
        let result = |duplicate| CaptureResult {
            accepted: true,
            duplicate,
            event_id: observation.event_id.clone(),
        };
        if let Some(hash) = self
            .observation_hash(person_id, &observation.event_id)
            .await?
        {
            return replay(hash == observation.content_hash, &observation.event_id)
                .map(|()| result(true));
        }
        let project_id = self
            .context_project(person_id, &observation.context)
            .await?;
        upsert_session(
            &self.pool,
            person_id,
            &observation.session_id,
            project_id.as_deref(),
            &observation.context,
            true,
        )
        .await?;
        let done = sqlx::query(
            "INSERT INTO observations (person_id, event_id, session_id, role, source, content, content_hash, context, captured_at, expires_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8, to_timestamp($9), to_timestamp($10))
             ON CONFLICT (person_id, event_id) DO NOTHING",
        )
        .bind(person_id)
        .bind(&observation.event_id)
        .bind(&observation.session_id)
        .bind(&observation.role)
        .bind(&observation.source)
        .bind(&observation.content)
        .bind(&observation.content_hash)
        .bind(serde_json::to_value(&observation.context).unwrap_or_default())
        .bind(observation.captured_at as f64)
        .bind(observation.expires_at as f64)
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            // Lost a race with the same event id.
            let hash = self
                .observation_hash(person_id, &observation.event_id)
                .await?
                .unwrap_or_default();
            return replay(hash == observation.content_hash, &observation.event_id)
                .map(|()| result(true));
        }
        Ok(result(false))
    }

    async fn observation_hash(&self, person_id: &str, event_id: &str) -> Result<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT content_hash FROM observations WHERE person_id = $1 AND event_id = $2",
        )
        .bind(person_id)
        .bind(event_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    async fn session_context(
        &self,
        person_id: &str,
        session_id: &str,
    ) -> Result<Option<WorkContext>> {
        let value: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT context FROM sessions WHERE person_id = $1 AND session_id = $2",
        )
        .bind(person_id)
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(value.and_then(|v| serde_json::from_value(v).ok()))
    }

    /// Ends a session and queues its distill job. False when there is nothing to distill.
    pub async fn end_session(&self, person_id: &str, session_id: &str) -> Result<bool> {
        self.queue_distill(person_id, session_id, true).await
    }

    /// Queues a distill job and keeps the session open.
    pub async fn checkpoint_session(&self, person_id: &str, session_id: &str) -> Result<bool> {
        self.queue_distill(person_id, session_id, false).await
    }

    async fn queue_distill(&self, person_id: &str, session_id: &str, end: bool) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let pending: Option<bool> = sqlx::query_scalar(
            "UPDATE sessions SET ended_at = CASE WHEN $3 THEN now() ELSE ended_at END
              WHERE person_id = $1 AND session_id = $2 RETURNING pending_since IS NOT NULL",
        )
        .bind(person_id)
        .bind(session_id)
        .bind(end)
        .fetch_optional(&mut *tx)
        .await?;
        let queued = if pending == Some(true) {
            enqueue_tx(
                &mut tx,
                JobKind::Distill,
                &distill_key(person_id, session_id),
            )
            .await?;
            true
        } else {
            false
        };
        tx.commit().await?;
        Ok(queued)
    }

    pub async fn session_observations(
        &self,
        person_id: &str,
        session_id: &str,
    ) -> Result<Vec<Observation>> {
        let rows = sqlx::query(
            "SELECT event_id, session_id, role, source, content, content_hash, context,
                    EXTRACT(EPOCH FROM captured_at)::bigint AS captured_at,
                    EXTRACT(EPOCH FROM expires_at)::bigint AS expires_at
               FROM observations
              WHERE person_id = $1 AND session_id = $2 AND expires_at > now()
              ORDER BY captured_at, event_id",
        )
        .bind(person_id)
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(Observation {
                    event_id: r.try_get("event_id")?,
                    session_id: r.try_get("session_id")?,
                    role: r.try_get("role")?,
                    source: r.try_get("source")?,
                    content: r.try_get("content")?,
                    content_hash: r.try_get("content_hash")?,
                    context: serde_json::from_value(r.try_get("context")?).unwrap_or_default(),
                    captured_at: r.try_get("captured_at")?,
                    expires_at: r.try_get("expires_at")?,
                })
            })
            .collect()
    }

    /// The `distill` job body: typed notes become personal memories, and the handoff for the
    /// session is written (or replaced) under a derived id. A session with no retained
    /// observation is an error whose text `is_empty_session` recognises.
    pub async fn distill_session(
        &self,
        person_id: &str,
        session_id: &str,
    ) -> Result<DistillReport> {
        let started: f64 =
            sqlx::query_scalar("SELECT EXTRACT(EPOCH FROM clock_timestamp())::float8")
                .fetch_one(&self.pool)
                .await?;
        let observations = self.session_observations(person_id, session_id).await?;
        let Some(last) = observations.last() else {
            return Err(StoreError::Validation(format!(
                "session has no retained observations: {session_id}"
            )));
        };
        let session_project: Option<String> = sqlx::query_scalar(
            "SELECT project_id FROM sessions WHERE person_id = $1 AND session_id = $2",
        )
        .bind(person_id)
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?
        .flatten();
        let work = WorkContext {
            project_id: session_project.clone(),
            ..last.context.clone()
        };
        let output = distill(&observations);
        let mut report = DistillReport::default();
        for note in output.notes {
            match self.save_as(person_id, note, &work, Origin::System).await {
                Ok(_) => report.notes += 1,
                // A note the person forgot stays forgotten.
                Err(StoreError::Conflict(_)) => {}
                Err(error) => return Err(error),
            }
        }
        if let Some(handoff) = output.handoff {
            let new = prepare_save(
                handoff,
                &work,
                session_project.is_some(),
                Origin::System,
                now_seconds(),
            )?;
            let id = handoff_id(person_id, session_id);
            let mut tx = self.pool.begin().await?;
            let status: Option<String> =
                sqlx::query_scalar("SELECT status FROM memories WHERE memory_id = $1 FOR UPDATE")
                    .bind(&id)
                    .fetch_optional(&mut *tx)
                    .await?;
            if status.as_deref() != Some("forgotten") {
                sqlx::query("DELETE FROM memories WHERE memory_id = $1 AND owner_id = $2")
                    .bind(&id)
                    .bind(person_id)
                    .execute(&mut *tx)
                    .await?;
                insert_memory(
                    &mut tx,
                    person_id,
                    session_project.as_deref(),
                    &new,
                    Some(&id),
                )
                .await?;
                report.handoff = true;
            }
            tx.commit().await?;
        }
        sqlx::query(
            "UPDATE sessions SET pending_since = NULL WHERE person_id = $1 AND session_id = $2
                AND NOT EXISTS (SELECT 1 FROM observations o WHERE o.person_id = $1 AND o.session_id = $2
                                   AND o.captured_at > to_timestamp($3))",
        )
        .bind(person_id)
        .bind(session_id)
        .bind(started)
        .execute(&self.pool)
        .await?;
        Ok(report)
    }

    /// Housekeeping, safe to run often: closes idle sessions with pending captures (queueing their
    /// distill), and hard-deletes memories 30 days past expiry, expired observations and old done jobs.
    pub async fn sweep(&self) -> Result<SweepReport> {
        let mut report = SweepReport::default();
        let mut tx = self.pool.begin().await?;
        let idle = sqlx::query(&format!(
            "UPDATE sessions s SET ended_at = now()
              WHERE s.ended_at IS NULL AND s.pending_since IS NOT NULL
                AND (SELECT max(o.captured_at) FROM observations o
                      WHERE o.person_id = s.person_id AND o.session_id = s.session_id)
                    < now() - make_interval(secs => {IDLE_SESSION_SECONDS})
              RETURNING s.person_id, s.session_id"
        ))
        .fetch_all(&mut *tx)
        .await?;
        for row in &idle {
            let key = distill_key(
                &row.try_get::<String, _>("person_id")?,
                &row.try_get::<String, _>("session_id")?,
            );
            enqueue_tx(&mut tx, JobKind::Distill, &key).await?;
        }
        report.sessions_closed = idle.len() as u64;
        report.memories_deleted = sqlx::query(&format!(
            "DELETE FROM memories WHERE expires_at < now() - make_interval(secs => {EXPIRED_GRACE_SECONDS})"
        ))
        .execute(&mut *tx)
        .await?
        .rows_affected();
        report.observations_deleted =
            sqlx::query("DELETE FROM observations WHERE expires_at < now()")
                .execute(&mut *tx)
                .await?
                .rows_affected();
        report.jobs_deleted = sqlx::query(&format!(
            "DELETE FROM jobs WHERE status = 'done' AND finished_at < now() - make_interval(secs => {JOB_DONE_RETENTION_SECONDS})"
        ))
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(report)
    }
}

fn replay(same: bool, event_id: &str) -> Result<()> {
    if same {
        Ok(())
    } else {
        Err(StoreError::Conflict(format!(
            "event id already used with different content: {event_id}"
        )))
    }
}

async fn upsert_session(
    pool: &sqlx::PgPool,
    person_id: &str,
    session_id: &str,
    project_id: Option<&str>,
    work: &WorkContext,
    pending: bool,
) -> Result<()> {
    // Only the git state is kept on the session; NULL means "nothing new to remember".
    let has_git = work.repository.is_some()
        || work.branch.is_some()
        || work.worktree.is_some()
        || work.commit.is_some();
    let context = has_git.then(|| {
        serde_json::json!({
            "repository": work.repository, "branch": work.branch,
            "worktree": work.worktree, "commit": work.commit,
        })
    });
    sqlx::query(
        "INSERT INTO sessions (person_id, session_id, project_id, context, pending_since)
         VALUES ($1, $2, $3, COALESCE($4::jsonb, '{}'::jsonb), CASE WHEN $5 THEN now() END)
         ON CONFLICT (person_id, session_id) DO UPDATE
            SET project_id = COALESCE(EXCLUDED.project_id, sessions.project_id),
                context = COALESCE($4::jsonb, sessions.context),
                ended_at = CASE WHEN $5 THEN NULL ELSE sessions.ended_at END,
                pending_since = CASE WHEN $5 THEN COALESCE(sessions.pending_since, now()) ELSE sessions.pending_since END",
    )
    .bind(person_id)
    .bind(session_id)
    .bind(project_id)
    .bind(context)
    .bind(pending)
    .execute(pool)
    .await?;
    Ok(())
}
