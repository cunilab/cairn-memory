use crate::jobs::enqueue_tx;
use crate::memory::insert_memory;
use crate::{Result, Store, StoreError};
use cairn_core::capture::session::IDLE_SESSION_SECONDS;
use cairn_core::capture::{
    CaptureResult, Observation, ObservationInput, WorkContext, normalize_observation,
};
use cairn_core::capture::{redact_sensitive_content, redact_work_context};
use cairn_core::id::now_seconds;
use cairn_core::memory::kind::EXPIRED_GRACE_SECONDS;
use cairn_core::process::distill::{distill, handoff_id};
use cairn_core::process::jobs::{JOB_DONE_RETENTION_SECONDS, JobKind, distill_key};
use cairn_core::process::{Origin, prepare_save};
use sqlx::Row;

pub(crate) const RECENT_FILES_MAX: usize = 20;
pub(crate) const RECENT_FILE_MAX_BYTES: usize = 512;

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
    /// Stores one redacted observation. The same event id with the same content, or the same
    /// content in the session under another event id, is a duplicate; an event id with other
    /// content is a 409.
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
        if let Some(duplicate) = self.known(person_id, &observation).await? {
            return duplicate.map(|()| result(true));
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
            &[],
            true,
        )
        .await?;
        let done = sqlx::query(
            // The capture time is `clock_timestamp()`, not the request's whole second: distillation reads the
            // order of a fast session (a failure, an edit, a pass) and event ids do not sort by time. The
            // expiry keeps the same distance from it.
            "INSERT INTO observations (person_id, event_id, session_id, role, source, kind, ok, content, content_hash, context, captured_at, expires_at)
             SELECT $1,$2,$3,$4,$5,$6,$7,$8,$9,$10, t.now, t.now + make_interval(secs => $11)
               FROM (SELECT clock_timestamp() AS now) t
             ON CONFLICT DO NOTHING",
        )
        .bind(person_id)
        .bind(&observation.event_id)
        .bind(&observation.session_id)
        .bind(&observation.role)
        .bind(&observation.source)
        .bind(observation.kind.as_str())
        .bind(observation.ok)
        .bind(&observation.content)
        .bind(&observation.content_hash)
        .bind(serde_json::to_value(&observation.context).unwrap_or_default())
        .bind((observation.expires_at - observation.captured_at) as f64)
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            // Lost a race with the same event id or the same content.
            if let Some(duplicate) = self.known(person_id, &observation).await? {
                return duplicate.map(|()| result(true));
            }
        }
        Ok(result(done.rows_affected() == 0))
    }

    /// `Some` when the capture is already stored under either key: `Ok` for the same event id
    /// with the same content or the same content in the session under another event id,
    /// a conflict for an event id used with other content.
    async fn known(
        &self,
        person_id: &str,
        observation: &Observation,
    ) -> Result<Option<Result<()>>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT event_id, content_hash FROM observations
              WHERE person_id = $1
                AND (event_id = $2 OR (session_id = $3 AND content_hash = $4))",
        )
        .bind(person_id)
        .bind(&observation.event_id)
        .bind(&observation.session_id)
        .bind(&observation.content_hash)
        .fetch_all(&self.pool)
        .await?;
        let same_event = rows.iter().find(|(id, _)| *id == observation.event_id);
        Ok(match (same_event, rows.is_empty()) {
            (Some((_, hash)), _) => Some(replay(
                *hash == observation.content_hash,
                &observation.event_id,
            )),
            (None, false) => Some(Ok(())),
            (None, true) => None,
        })
    }

    /// Opens (or touches) the session a brief was served to and keeps the repo-relative files
    /// the hook saw, redacted, at most 20, for step 5's ranking. Nothing is pending.
    pub async fn record_session_files(
        &self,
        person_id: &str,
        session_id: &str,
        work: &WorkContext,
        recent_files: &[String],
    ) -> Result<()> {
        if session_id.trim().is_empty() {
            return Err(StoreError::Validation(
                "session_id must not be empty".into(),
            ));
        }
        let files: Vec<String> = recent_files
            .iter()
            .take(RECENT_FILES_MAX)
            .map(|file| clip(&redact_sensitive_content(file), RECENT_FILE_MAX_BYTES))
            .collect();
        upsert_session(
            &self.pool,
            person_id,
            session_id,
            None,
            &redact_work_context(work),
            &files,
            false,
        )
        .await
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
            "SELECT event_id, session_id, role, source, kind, ok, content, content_hash, context,
                    EXTRACT(EPOCH FROM captured_at)::bigint AS captured_at,
                    EXTRACT(EPOCH FROM expires_at)::bigint AS expires_at
               FROM observations
              WHERE person_id = $1 AND session_id = $2 AND expires_at > now()
              ORDER BY observations.captured_at, event_id",
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
                    kind: r
                        .try_get::<String, _>("kind")?
                        .parse()
                        .map_err(StoreError::Validation)?,
                    ok: r.try_get("ok")?,
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
        // The checkout only; each note adds the files it is about.
        let work = WorkContext {
            project_id: session_project.clone(),
            repository: last.context.repository.clone(),
            branch: last.context.branch.clone(),
            worktree: last.context.worktree.clone(),
            commit: last.context.commit.clone(),
            ..WorkContext::default()
        };
        let output = distill(&observations);
        let mut report = DistillReport::default();
        for note in output.notes {
            let note_work = WorkContext {
                files: note.files,
                ..work.clone()
            };
            match self
                .save_as(person_id, note.input, &note_work, Origin::System)
                .await
            {
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
            let id = handoff_id(person_id, &work);
            // The id is fixed per checkout. Forgetting a handoff forgets that content, not the
            // checkout: the same content stays forgotten, a later handoff replaces the row.
            let mut tx = self.pool.begin().await?;
            let forgotten: Option<String> = sqlx::query_scalar(
                "SELECT identity_hash FROM memories WHERE memory_id = $1 AND status = 'forgotten' FOR UPDATE",
            )
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await?;
            if forgotten.as_deref() != Some(new.identity_hash.as_str()) {
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
        // A system-derived memory nobody has used since it went stale lapses after 90 days. An
        // agent's memory never does: stale only ranks it lower and shows why.
        sqlx::query(
            "UPDATE memories m SET expires_at = now()
              WHERE m.origin = 'system' AND m.status = 'stale' AND m.status_changed_at < now() - interval '90 days'
                AND m.expires_at IS NULL
                AND NOT EXISTS (SELECT 1 FROM usage u WHERE u.memory_id = m.memory_id
                                   AND u.last_used_at > m.status_changed_at)",
        )
        .execute(&mut *tx)
        .await?;
        report.observations_deleted =
            sqlx::query("DELETE FROM observations WHERE expires_at < now()")
                .execute(&mut *tx)
                .await?
                .rows_affected();
        // Distilled sessions keep their raw captures for a week.
        report.observations_deleted += sqlx::query(
            "DELETE FROM observations o USING sessions s
              WHERE o.person_id = s.person_id AND o.session_id = s.session_id
                AND s.pending_since IS NULL AND s.ended_at < now() - interval '7 days'",
        )
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
        // A failed retry must not stop the reconcile pass.
        let retried = self.retry_quarantined().await;
        self.reconcile_promoted().await?;
        retried?;
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
    recent_files: &[String],
    pending: bool,
) -> Result<()> {
    // Only the git state and the recent files are kept on the session; a later write merges
    // over the keys it carries and NULL means "nothing new to remember".
    let has_git = work.repository.is_some()
        || work.branch.is_some()
        || work.worktree.is_some()
        || work.commit.is_some();
    let mut context = serde_json::Map::new();
    if has_git {
        context.insert("repository".into(), work.repository.clone().into());
        context.insert("branch".into(), work.branch.clone().into());
        context.insert("worktree".into(), work.worktree.clone().into());
        context.insert("commit".into(), work.commit.clone().into());
    }
    if !recent_files.is_empty() {
        context.insert("recent_files".into(), recent_files.into());
    }
    let context = (!context.is_empty()).then_some(serde_json::Value::Object(context));
    sqlx::query(
        "INSERT INTO sessions (person_id, session_id, project_id, context, pending_since)
         VALUES ($1, $2, $3, COALESCE($4::jsonb, '{}'::jsonb), CASE WHEN $5 THEN now() END)
         ON CONFLICT (person_id, session_id) DO UPDATE
            SET project_id = COALESCE(EXCLUDED.project_id, sessions.project_id),
                context = sessions.context || COALESCE($4::jsonb, '{}'::jsonb),
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

pub(crate) fn clip(text: &str, max_bytes: usize) -> String {
    let mut end = text.len().min(max_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}
