use crate::jobs::enqueue_tx;
use crate::memory::insert_memory;
use crate::usage::RETRIEVAL_RETENTION_DAYS;
use crate::{Result, Store, StoreError};
use cairn_core::capture::session::IDLE_SESSION_SECONDS;
use cairn_core::capture::{
    CaptureKind, CaptureResult, Observation, ObservationInput, WorkContext, normalize_observation,
};
use cairn_core::capture::{redact_sensitive_content, redact_work_context};
use cairn_core::id::now_seconds;
use cairn_core::memory::{Fields, Scope};
use cairn_core::process::distill::distill;

const HANDOFF_CHAIN_ORIGIN: &str = "system";
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
    /// The new handoff is thin and an older one in the chain has files or open questions, so the
    /// brief keeps that older one.
    pub handoff_kept: bool,
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
        self.capture_from(person_id, input, None).await
    }

    /// `capture`, remembering the device that sent it (a distilled session names it).
    pub async fn capture_from(
        &self,
        person_id: &str,
        input: ObservationInput,
        device_id: Option<&str>,
    ) -> Result<CaptureResult> {
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
            &observation.context,
            SessionTouch {
                project_id: project_id.as_deref(),
                recent_files: &[],
                pending: true,
                device_id,
            },
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
            &redact_work_context(work),
            SessionTouch {
                project_id: None,
                recent_files: &files,
                pending: false,
                device_id: None,
            },
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
        let res: Option<(bool, bool)> = sqlx::query_as(
            "WITH prev AS (
                 SELECT person_id, session_id, ended_at, pending_since
                   FROM sessions
                  WHERE person_id = $1 AND session_id = $2
                    FOR UPDATE
             )
             UPDATE sessions s
                SET ended_at = CASE WHEN $3 AND prev.ended_at IS NULL THEN now() ELSE s.ended_at END
               FROM prev
              WHERE s.person_id = prev.person_id AND s.session_id = prev.session_id
             RETURNING prev.pending_since IS NOT NULL AS pending,
                       prev.ended_at IS NULL AS was_open",
        )
        .bind(person_id)
        .bind(session_id)
        .bind(end)
        .fetch_optional(&mut *tx)
        .await?;
        let queued = match res {
            Some((pending, was_open)) => {
                let should_queue = if end { was_open } else { pending };
                if should_queue {
                    enqueue_tx(
                        &mut tx,
                        JobKind::Distill,
                        &distill_key(person_id, session_id),
                    )
                    .await?;
                    true
                } else {
                    false
                }
            }
            None => false,
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

    /// The `distill` job body: typed notes become memories, and the checkout's handoff is a new
    /// row linked to the previous one. A session with no retained observation is an error whose
    /// text `is_empty_session` recognises.
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
        let (session_project, session_device, session_context, session_ended): (
            Option<String>,
            Option<String>,
            serde_json::Value,
            bool,
        ) = sqlx::query_as(
            "SELECT project_id, device_id, context, ended_at IS NOT NULL FROM sessions WHERE person_id = $1 AND session_id = $2",
        )
        .bind(person_id)
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?
        .unwrap_or_default();
        let automated = session_context
            .get("origin")
            .and_then(|value| value.as_str())
            == Some("automation");
        let device = session_device.as_deref();
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
        let mut written: Vec<String> = Vec::new();
        let note_scope = if session_project.is_some() {
            Scope::Project
        } else {
            Scope::Personal
        };
        let note_count = output.notes.len();
        let mut notes_created = 0;
        let mut notes_deduplicated = 0;
        let mut notes_forgotten = 0;
        for mut note in output.notes {
            note.input.scope = Some(note_scope);
            let note_work = WorkContext {
                files: note.files,
                ..work.clone()
            };
            match self
                .save_as(person_id, note.input, &note_work, Origin::System, device)
                .await
            {
                Ok(saved) => {
                    report.notes += 1;
                    if saved.deduplicated {
                        notes_deduplicated += 1;
                    } else {
                        notes_created += 1;
                    }
                    written.push(saved.id);
                }
                // A note the person forgot stays forgotten.
                Err(StoreError::Conflict(_)) => notes_forgotten += 1,
                Err(error) => return Err(error),
            }
        }
        let mut digest_written_flag = false;
        if let Some(mut digest) = output.digest.filter(|_| session_ended) {
            digest.scope = Some(note_scope);
            let mut files = Vec::new();
            for observation in &observations {
                if observation.kind != CaptureKind::Edit {
                    continue;
                }
                for file in &observation.context.files {
                    if !file.is_empty() && !files.contains(file) {
                        files.push(file.clone());
                    }
                }
            }
            let digest_work = WorkContext {
                files,
                ..work.clone()
            };
            match self
                .save_as(person_id, digest, &digest_work, Origin::System, device)
                .await
            {
                Ok(saved) => {
                    written.push(saved.id);
                    digest_written_flag = !saved.deduplicated;
                }
                Err(StoreError::Conflict(_)) => {}
                Err(error) => return Err(error),
            }
        }
        let digest_written = session_ended.then_some(digest_written_flag);
        let mut handoff_written = None;
        let mut kept_id = None;
        let mut handoff_outcome_val = None;
        let had_handoff = output.handoff.is_some();
        // An automated session keeps its notes and leaves the checkout's handoff alone.
        if let Some(handoff) = output.handoff.filter(|_| session_ended && !automated) {
            let new = prepare_save(
                handoff,
                &work,
                session_project.is_some(),
                Origin::System,
                now_seconds(),
            )?;
            // Each handoff is its own row. The previous one of this checkout (same person, repo,
            // branch, worktree and project) is superseded and linked newer → older.
            let mut tx = self.pool.begin().await?;
            let previous = sqlx::query(
                "SELECT memory_id, status, fields FROM memories
                  WHERE owner_id = $1 AND kind = 'handoff'
                    AND repo IS NOT DISTINCT FROM $2 AND repo_branch IS NOT DISTINCT FROM $3
                    AND repo_worktree IS NOT DISTINCT FROM $4
                    AND project_id IS NOT DISTINCT FROM $5
                  ORDER BY created_at DESC, memory_id DESC LIMIT 1 FOR UPDATE",
            )
            .bind(person_id)
            .bind(&new.context.repo)
            .bind(&new.context.branch)
            .bind(&new.context.worktree)
            .bind(session_project.as_deref())
            .fetch_optional(&mut *tx)
            .await?;
            let kept = match &previous {
                Some(row) => {
                    let fields: serde_json::Value = row.try_get("fields")?;
                    matches!(
                        row.try_get::<String, _>("status")?.as_str(),
                        "active" | "stale"
                    ) && (json_list_nonempty(&fields, "files")
                        || json_list_nonempty(&fields, "open_questions"))
                        && handoff_is_thin(&new.fields)
                }
                None => false,
            };
            match insert_memory(
                &mut tx,
                person_id,
                session_project.as_deref(),
                &new,
                None,
                device,
            )
            .await
            {
                Ok(saved) => {
                    if !saved.deduplicated {
                        if let Some(row) = &previous {
                            let old: String = row.try_get("memory_id")?;
                            if old != saved.id {
                                sqlx::query(&format!(
                                    "INSERT INTO links (source_id, target_id, relation, origin)
                                     VALUES ($1, $2, 'supersedes', '{HANDOFF_CHAIN_ORIGIN}')
                                     ON CONFLICT (source_id, target_id, relation) DO NOTHING"
                                ))
                                .bind(&saved.id)
                                .bind(&old)
                                .execute(&mut *tx)
                                .await?;
                                sqlx::query(
                                    "UPDATE memories SET status = 'superseded', stale_cause = NULL,
                                            status_changed_at = now(), updated_at = now()
                                      WHERE memory_id = $1 AND status IN ('active','stale')",
                                )
                                .bind(&old)
                                .execute(&mut *tx)
                                .await?;
                            }
                        }
                        report.handoff = true;
                        handoff_written = Some(saved.id);
                        handoff_outcome_val = Some("written");
                    } else {
                        handoff_outcome_val = Some("kept");
                    }
                    if kept && !saved.deduplicated {
                        report.handoff_kept = true;
                        kept_id = previous.as_ref().map(|row| row.get("memory_id"));
                    }
                    Ok(())
                }
                // The same content was forgotten; it stays forgotten.
                Err(StoreError::Conflict(_)) => {
                    handoff_outcome_val = Some("none");
                    Ok(())
                }
                Err(error) => Err(error),
            }?;
            tx.commit().await?;
        }
        let handoff_outcome: Option<&str> = if session_ended {
            if automated {
                Some("skipped")
            } else if !had_handoff {
                Some("none")
            } else {
                handoff_outcome_val.or(Some("none"))
            }
        } else {
            None
        };
        let mut changed_files = Vec::new();
        let mut outside_count = 0;
        for o in &observations {
            if o.kind == CaptureKind::Edit {
                for f in &o.context.files {
                    let trimmed = f.trim();
                    if !trimmed.is_empty() && !changed_files.iter().any(|c| c == trimmed) {
                        changed_files.push(trimmed.to_owned());
                    }
                }
                if o.context.outside_files > outside_count {
                    outside_count = o.context.outside_files;
                }
            }
        }
        let files_changed = (changed_files.len() + outside_count as usize) as i32;
        let why_none: Option<Vec<String>> = if notes_created == 0 {
            let mut reasons: Vec<String> = output.why_none.iter().map(|&s| s.to_owned()).collect();
            if note_count > 0 && notes_deduplicated == note_count {
                reasons.push("all_duplicates".to_owned());
            }
            if note_count > 0 && notes_forgotten == note_count {
                reasons.push("all_forgotten".to_owned());
            }
            if automated && !reasons.iter().any(|r| r == "automated") {
                reasons.push("automated".to_owned());
            }
            (!reasons.is_empty()).then_some(reasons)
        } else {
            None
        };
        sqlx::query(
            "UPDATE sessions
                SET distilled_at = now(),
                    files_changed = $3,
                    notes_written = COALESCE(notes_written, 0) + $4,
                    digest_written = CASE WHEN $5 THEN $6 ELSE digest_written END,
                    handoff_outcome = CASE WHEN $5 THEN $7 ELSE handoff_outcome END,
                    why_none = CASE WHEN COALESCE(notes_written, 0) + $4 > 0 THEN NULL ELSE $8 END
              WHERE person_id = $1 AND session_id = $2",
        )
        .bind(person_id)
        .bind(session_id)
        .bind(files_changed)
        .bind(notes_created)
        .bind(session_ended)
        .bind(digest_written)
        .bind(handoff_outcome)
        .bind(why_none.as_deref())
        .execute(&self.pool)
        .await?;
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
        if report.handoff || report.notes > 0 || report.handoff_kept {
            let recorded = handoff_written.clone().or(kept_id);
            let ids: Vec<&String> = recorded.iter().chain(&written).collect();
            self.record_audit(
                person_id,
                None,
                "session_distilled",
                recorded.as_deref().or(written.first().map(String::as_str)),
                serde_json::json!({
                    "handoff_id": recorded,
                    "handoff_kept": report.handoff_kept,
                    "branch": work.branch,
                    "notes": report.notes,
                    "device_id": session_device,
                    "ids": ids,
                    "automated": automated,
                }),
            )
            .await?;
        }
        Ok(report)
    }

    /// Housekeeping, safe to run often: closes idle sessions with pending captures (queueing their
    /// distill), and deletes expired observations, old done jobs and old retrieval-log rows.
    /// Memories are never deleted.
    pub async fn sweep(&self) -> Result<SweepReport> {
        let mut report = SweepReport::default();
        let mut tx = self.pool.begin().await?;
        let idle = sqlx::query(&format!(
            "UPDATE sessions s SET ended_at = now()
              WHERE s.ended_at IS NULL
                AND EXISTS (SELECT 1 FROM observations o
                             WHERE o.person_id = s.person_id AND o.session_id = s.session_id
                               AND o.captured_at > now() - interval '48 hours')
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
        sqlx::query(&format!(
            "DELETE FROM retrieval_log WHERE bucket < now() - interval '{RETRIEVAL_RETENTION_DAYS} days'"
        ))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        // A failed retry must not stop the reconcile pass.
        let retried = self.retry_quarantined().await;
        self.reconcile_promoted().await?;
        retried?;
        Ok(report)
    }
}

fn json_list_nonempty(fields: &serde_json::Value, key: &str) -> bool {
    fields
        .get(key)
        .and_then(serde_json::Value::as_array)
        .is_some_and(|items| !items.is_empty())
}

fn handoff_is_thin(fields: &Fields) -> bool {
    match fields {
        Fields::Handoff(handoff) => handoff.files.is_empty() && handoff.open_questions.is_empty(),
        _ => false,
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

/// What a write to a session carries besides its git state.
struct SessionTouch<'a> {
    project_id: Option<&'a str>,
    recent_files: &'a [String],
    /// A capture (not a brief) leaves distillation pending.
    pending: bool,
    device_id: Option<&'a str>,
}

async fn upsert_session(
    pool: &sqlx::PgPool,
    person_id: &str,
    session_id: &str,
    work: &WorkContext,
    touch: SessionTouch<'_>,
) -> Result<()> {
    let SessionTouch {
        project_id,
        recent_files,
        pending,
        device_id,
    } = touch;
    // Git state, recent files and an automation origin are kept on the session; a later write
    // merges over the keys it carries and NULL means "nothing new to remember".
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
    if let Some(origin) = work.origin.as_deref().filter(|origin| !origin.is_empty()) {
        context.insert("origin".into(), origin.into());
    }
    let context = (!context.is_empty()).then_some(serde_json::Value::Object(context));
    sqlx::query(
        "INSERT INTO sessions (person_id, session_id, project_id, context, pending_since, device_id)
         VALUES ($1, $2, $3, COALESCE($4::jsonb, '{}'::jsonb), CASE WHEN $5 THEN now() END, $6)
         ON CONFLICT (person_id, session_id) DO UPDATE
            SET project_id = COALESCE(EXCLUDED.project_id, sessions.project_id),
                device_id = COALESCE(EXCLUDED.device_id, sessions.device_id),
                context = sessions.context || COALESCE($4::jsonb, '{}'::jsonb),
                ended_at = CASE WHEN $5 THEN NULL ELSE sessions.ended_at END,
                pending_since = CASE WHEN $5 THEN COALESCE(sessions.pending_since, now()) ELSE sessions.pending_since END",
    )
    .bind(person_id)
    .bind(session_id)
    .bind(project_id)
    .bind(context)
    .bind(pending)
    .bind(device_id)
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
