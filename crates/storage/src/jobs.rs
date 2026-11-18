use super::*;
use cairn_core::{
    BackgroundJob, JobEnd, JobKind, JobSnapshot, QuarantinedJob, distill_job_end, distill_key,
    parse_distill_key,
};
use sqlx::{Postgres, Transaction};

const JOB_COLUMNS: &str = "job_id, kind, job_key, attempts, not_before, claim_id, lease_expires_at, last_error, created_at, updated_at, finished_at, succeeded";

fn stamp(value: u64) -> Result<i64, PostgresError> {
    i64::try_from(value)
        .map_err(|_| PostgresError::Validation("job timestamp is out of range".into()))
}

fn count(value: i32) -> Result<u32, PostgresError> {
    u32::try_from(value)
        .map_err(|_| PostgresError::Validation("job attempt count is out of range".into()))
}

fn job_from_row(row: &PgRow) -> Result<BackgroundJob, PostgresError> {
    Ok(BackgroundJob {
        job_id: row.try_get("job_id")?,
        kind: row.try_get("kind")?,
        key: row.try_get("job_key")?,
        attempts: count(row.try_get("attempts")?)?,
        not_before: u64::try_from(row.try_get::<i64, _>("not_before")?).unwrap_or(0),
        claim_id: row.try_get("claim_id")?,
        lease_expires_at: row
            .try_get::<Option<i64>, _>("lease_expires_at")?
            .map(|value| u64::try_from(value).unwrap_or(0)),
        last_error: row.try_get("last_error")?,
        created_at: u64::try_from(row.try_get::<i64, _>("created_at")?).unwrap_or(0),
        updated_at: u64::try_from(row.try_get::<i64, _>("updated_at")?).unwrap_or(0),
        finished_at: row
            .try_get::<Option<i64>, _>("finished_at")?
            .map(|value| u64::try_from(value).unwrap_or(0)),
        succeeded: row.try_get("succeeded")?,
    })
}

fn quarantine_from_row(row: &PgRow) -> Result<QuarantinedJob, PostgresError> {
    Ok(QuarantinedJob {
        job_id: row.try_get("job_id")?,
        kind: row.try_get("kind")?,
        key: row.try_get("job_key")?,
        attempts: count(row.try_get("attempts")?)?,
        last_error: row.try_get("last_error")?,
        created_at: u64::try_from(row.try_get::<i64, _>("created_at")?).unwrap_or(0),
        failed_at: u64::try_from(row.try_get::<i64, _>("failed_at")?).unwrap_or(0),
        quarantined_at: u64::try_from(row.try_get::<i64, _>("quarantined_at")?).unwrap_or(0),
    })
}

pub(crate) async fn enqueue_on(
    tx: &mut Transaction<'_, Postgres>,
    kind: JobKind,
    key: &str,
    now: u64,
    fresh: bool,
) -> Result<bool, PostgresError> {
    let kind_name = kind.as_str();
    let now_stamp = stamp(now)?;
    if !fresh {
        let inserted = sqlx::query_scalar::<_, String>(
            "INSERT INTO cairn_jobs
                (job_id, kind, job_key, attempts, not_before, claim_id, lease_expires_at, last_error, created_at, updated_at, finished_at, succeeded)
             SELECT $1, $2, $3, 0, $4, NULL, NULL, NULL, $4, $4, NULL, false
             WHERE NOT EXISTS (
                 SELECT 1 FROM cairn_jobs WHERE kind = $2 AND job_key = $3 AND succeeded = false
             )
             AND NOT EXISTS (
                 SELECT 1 FROM cairn_job_quarantine WHERE kind = $2 AND job_key = $3
             )
             ON CONFLICT (kind, job_key) WHERE succeeded = false AND claim_id IS NULL DO NOTHING
             RETURNING job_id",
        )
        .bind(random_token("job"))
        .bind(kind_name)
        .bind(key)
        .bind(now_stamp)
        .fetch_optional(&mut **tx)
        .await?;
        return Ok(inserted.is_some());
    }
    let locked = sqlx::query(
        "SELECT job_id, claim_id, lease_expires_at FROM cairn_jobs
         WHERE kind = $1 AND job_key = $2 AND succeeded = false
         ORDER BY job_id
         FOR UPDATE",
    )
    .bind(kind_name)
    .bind(key)
    .fetch_all(&mut **tx)
    .await?;
    let mut removed = false;
    for row in &locked {
        let claim_id: Option<String> = row.try_get("claim_id")?;
        let lease: Option<i64> = row.try_get("lease_expires_at")?;
        let live = claim_id.is_some() && lease.unwrap_or(0) > now_stamp;
        if !live {
            let job_id: String = row.try_get("job_id")?;
            sqlx::query("DELETE FROM cairn_jobs WHERE job_id = $1")
                .bind(job_id)
                .execute(&mut **tx)
                .await?;
            removed = true;
        }
    }
    let pending: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM cairn_jobs
             WHERE kind = $1 AND job_key = $2 AND succeeded = false AND claim_id IS NULL
         )",
    )
    .bind(kind_name)
    .bind(key)
    .fetch_one(&mut **tx)
    .await?;
    if pending {
        return Ok(removed);
    }
    sqlx::query(
        "INSERT INTO cairn_jobs
            (job_id, kind, job_key, attempts, not_before, claim_id, lease_expires_at, last_error, created_at, updated_at, finished_at, succeeded)
         VALUES ($1, $2, $3, 0, $4, NULL, NULL, NULL, $4, $4, NULL, false)",
    )
    .bind(random_token("job"))
    .bind(kind_name)
    .bind(key)
    .bind(now_stamp)
    .execute(&mut **tx)
    .await?;
    Ok(true)
}

async fn maintain_on(tx: &mut Transaction<'_, Postgres>, now: u64) -> Result<bool, PostgresError> {
    let cutoff = stamp(now.saturating_sub(cairn_core::JOB_SUCCEEDED_RETENTION_SECONDS))?;
    let deleted = sqlx::query(
        "DELETE FROM cairn_jobs
         WHERE succeeded = true AND finished_at IS NOT NULL AND finished_at <= $1",
    )
    .bind(cutoff)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    let expired = sqlx::query(
        "SELECT job_id, claim_id FROM cairn_jobs
         WHERE succeeded = false AND claim_id IS NOT NULL AND lease_expires_at <= $1
         ORDER BY job_id
         FOR UPDATE",
    )
    .bind(stamp(now)?)
    .fetch_all(&mut **tx)
    .await?;
    let mut changed = deleted > 0;
    for row in expired {
        let job_id: String = row.try_get("job_id")?;
        let claim_id: String = row.try_get("claim_id")?;
        changed |= fail_on(tx, &job_id, &claim_id, "lease expired", false, now).await?;
    }
    Ok(changed)
}

async fn claim_on(
    tx: &mut Transaction<'_, Postgres>,
    kind: &str,
    key: Option<&str>,
    limit: usize,
    now: u64,
) -> Result<Vec<BackgroundJob>, PostgresError> {
    let now_stamp = stamp(now)?;
    let lease = stamp(now.saturating_add(cairn_core::JOB_LEASE_SECONDS))?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let ids = sqlx::query_scalar::<_, String>(
        "SELECT j.job_id FROM cairn_jobs j
         WHERE j.kind = $1
           AND j.succeeded = false
           AND j.claim_id IS NULL
           AND j.not_before <= $2
           AND ($3::text IS NULL OR j.job_key = $3)
           AND NOT EXISTS (
               SELECT 1 FROM cairn_jobs live
               WHERE live.kind = j.kind
                 AND live.job_key = j.job_key
                 AND live.succeeded = false
                 AND live.claim_id IS NOT NULL
                 AND live.lease_expires_at > $2
           )
         ORDER BY j.not_before, j.job_id
         LIMIT $4
         FOR UPDATE OF j SKIP LOCKED",
    )
    .bind(kind)
    .bind(now_stamp)
    .bind(key)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;
    let mut claimed = Vec::with_capacity(ids.len());
    for job_id in ids {
        let row = sqlx::query(&format!(
            "UPDATE cairn_jobs
             SET claim_id = $2, lease_expires_at = $3, updated_at = $4
             WHERE job_id = $1
             RETURNING {JOB_COLUMNS}"
        ))
        .bind(&job_id)
        .bind(random_token("claim"))
        .bind(lease)
        .bind(now_stamp)
        .fetch_one(&mut **tx)
        .await?;
        claimed.push(job_from_row(&row)?);
    }
    Ok(claimed)
}

pub(crate) async fn settle_on(
    tx: &mut Transaction<'_, Postgres>,
    job_id: &str,
    claim_id: &str,
    now: u64,
    end: JobEnd,
) -> Result<bool, PostgresError> {
    match end {
        JobEnd::Succeeded => complete_on(tx, job_id, claim_id, now).await,
        JobEnd::Retry(error) => fail_on(tx, job_id, claim_id, &error, false, now).await,
        JobEnd::Quarantine(error) => fail_on(tx, job_id, claim_id, &error, true, now).await,
    }
}

async fn complete_on(
    tx: &mut Transaction<'_, Postgres>,
    job_id: &str,
    claim_id: &str,
    now: u64,
) -> Result<bool, PostgresError> {
    let now_stamp = stamp(now)?;
    let updated = sqlx::query(
        "UPDATE cairn_jobs
         SET succeeded = true, claim_id = NULL, lease_expires_at = NULL, last_error = NULL,
             finished_at = $3, updated_at = $3
         WHERE job_id = $1 AND claim_id = $2 AND succeeded = false",
    )
    .bind(job_id)
    .bind(claim_id)
    .bind(now_stamp)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// `false` when this claim does not own the row.
async fn fail_on(
    tx: &mut Transaction<'_, Postgres>,
    job_id: &str,
    claim_id: &str,
    error: &str,
    permanent: bool,
    now: u64,
) -> Result<bool, PostgresError> {
    let row = sqlx::query(&format!(
        "SELECT {JOB_COLUMNS} FROM cairn_jobs WHERE job_id = $1 FOR UPDATE"
    ))
    .bind(job_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let job = job_from_row(&row)?;
    if job.succeeded || job.claim_id.as_deref() != Some(claim_id) {
        return Ok(false);
    }
    let attempts = job.attempts.saturating_add(1);
    let now_stamp = stamp(now)?;
    if permanent || attempts >= cairn_core::JOB_ATTEMPT_CAP {
        sqlx::query("DELETE FROM cairn_jobs WHERE job_id = $1")
            .bind(job_id)
            .execute(&mut **tx)
            .await?;
        sqlx::query(
            "INSERT INTO cairn_job_quarantine
                (job_id, kind, job_key, attempts, last_error, created_at, failed_at, quarantined_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $7)",
        )
        .bind(job_id)
        .bind(&job.kind)
        .bind(&job.key)
        .bind(i32::try_from(attempts).unwrap_or(i32::MAX))
        .bind(error)
        .bind(stamp(job.created_at)?)
        .bind(now_stamp)
        .execute(&mut **tx)
        .await?;
        if job.kind == JobKind::Distill.as_str() {
            clear_covered_pending(tx, &job.key, now).await?;
        }
        return Ok(true);
    }
    let successor: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM cairn_jobs
             WHERE kind = $1 AND job_key = $2 AND succeeded = false AND claim_id IS NULL AND job_id <> $3
         )",
    )
    .bind(&job.kind)
    .bind(&job.key)
    .bind(job_id)
    .fetch_one(&mut **tx)
    .await?;
    if successor {
        sqlx::query("DELETE FROM cairn_jobs WHERE job_id = $1")
            .bind(job_id)
            .execute(&mut **tx)
            .await?;
        return Ok(true);
    }
    let not_before =
        stamp(now.saturating_add(cairn_core::JOB_BACKOFF_SECONDS[(attempts - 1) as usize]))?;
    sqlx::query(
        "UPDATE cairn_jobs
         SET attempts = $2, not_before = $3, claim_id = NULL, lease_expires_at = NULL,
             last_error = $4, updated_at = $5
         WHERE job_id = $1",
    )
    .bind(job_id)
    .bind(i32::try_from(attempts).unwrap_or(i32::MAX))
    .bind(not_before)
    .bind(error)
    .bind(now_stamp)
    .execute(&mut **tx)
    .await?;
    Ok(true)
}

/// Drops the pending mark only when no observation arrived after the failed attempt's view.
async fn clear_covered_pending(
    tx: &mut Transaction<'_, Postgres>,
    key: &str,
    covered_until: u64,
) -> Result<(), PostgresError> {
    let Some((Some(person_id), session_id)) = parse_distill_key(key) else {
        return Ok(());
    };
    sqlx::query(
        "UPDATE cairn_sessions SET pending_since = NULL
         WHERE person_id = $1 AND session_id = $2
           AND NOT EXISTS (
               SELECT 1 FROM cairn_observations observation
               WHERE observation.person_id = $1
                 AND observation.session_id = $2
                 AND observation.captured_at > to_timestamp($3::double precision)
           )",
    )
    .bind(person_id)
    .bind(session_id)
    .bind(covered_until as f64)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn claim_is_held(
    tx: &mut Transaction<'_, Postgres>,
    job_id: &str,
    claim_id: &str,
) -> Result<bool, PostgresError> {
    let held = sqlx::query_scalar::<_, String>(
        "SELECT job_id FROM cairn_jobs
         WHERE job_id = $1 AND claim_id = $2 AND succeeded = false
         FOR UPDATE",
    )
    .bind(job_id)
    .bind(claim_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(held.is_some())
}

impl PostgresStore {
    pub async fn enqueue_job(
        &self,
        kind: JobKind,
        key: &str,
        now: u64,
        fresh: bool,
    ) -> Result<bool, PostgresError> {
        let mut tx = self.pool.begin().await?;
        let inserted = enqueue_on(&mut tx, kind, key, now, fresh).await?;
        tx.commit().await?;
        Ok(inserted)
    }

    pub async fn poll_jobs(
        &self,
        kind: JobKind,
        limit: usize,
        now: u64,
    ) -> Result<Vec<BackgroundJob>, PostgresError> {
        let missing = if kind == JobKind::Embed {
            self.embedding_jobs(limit).await?
        } else {
            Vec::new()
        };
        let mut tx = self.pool.begin().await?;
        maintain_on(&mut tx, now).await?;
        for memory in &missing {
            enqueue_on(&mut tx, JobKind::Embed, &memory.memory_id, now, false).await?;
        }
        let claimed = claim_on(&mut tx, kind.as_str(), None, limit, now).await?;
        tx.commit().await?;
        Ok(claimed)
    }

    pub async fn settle_job(
        &self,
        job_id: &str,
        claim_id: &str,
        now: u64,
        end: JobEnd,
    ) -> Result<bool, PostgresError> {
        let mut tx = self.pool.begin().await?;
        let applied = settle_on(&mut tx, job_id, claim_id, now, end).await?;
        tx.commit().await?;
        Ok(applied)
    }

    pub async fn job_snapshot(&self, now: u64) -> Result<JobSnapshot, PostgresError> {
        let now_stamp = stamp(now)?;
        let (pending, running): (i64, i64) = sqlx::query_as(
            "SELECT
                count(*) FILTER (
                    WHERE succeeded = false
                      AND (claim_id IS NULL OR lease_expires_at IS NULL OR lease_expires_at <= $1)
                ),
                count(*) FILTER (
                    WHERE succeeded = false AND claim_id IS NOT NULL AND lease_expires_at > $1
                )
             FROM cairn_jobs",
        )
        .bind(now_stamp)
        .fetch_one(&self.pool)
        .await?;
        let rows = sqlx::query(
            "SELECT job_id, kind, job_key, attempts, last_error, created_at, failed_at, quarantined_at
             FROM cairn_job_quarantine",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut quarantine = rows
            .iter()
            .map(quarantine_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        quarantine.sort_by(|left, right| {
            right
                .quarantined_at
                .cmp(&left.quarantined_at)
                .then(left.job_id.cmp(&right.job_id))
        });
        Ok(JobSnapshot {
            pending: usize::try_from(pending).unwrap_or(usize::MAX),
            running: usize::try_from(running).unwrap_or(usize::MAX),
            quarantined: quarantine.len(),
            quarantine,
        })
    }

    pub async fn admin_jobs(&self, token: &str, now: u64) -> Result<JobSnapshot, PostgresError> {
        self.authenticate_admin(token).await?;
        self.job_snapshot(now).await
    }

    pub async fn requeue_job(&self, job_id: &str, now: u64) -> Result<(), PostgresError> {
        let mut tx = self.pool.begin().await?;
        if let Err(error) = requeue_on(&mut tx, job_id, now).await {
            tx.rollback().await?;
            return Err(error);
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn discard_job(&self, job_id: &str) -> Result<(), PostgresError> {
        let deleted = sqlx::query("DELETE FROM cairn_job_quarantine WHERE job_id = $1")
            .bind(job_id)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if deleted == 0 {
            return Err(PostgresError::Validation(
                "unknown quarantined job".to_owned(),
            ));
        }
        Ok(())
    }

    pub(super) async fn run_distill(
        &self,
        person_id: &str,
        session_id: &str,
        fresh: bool,
    ) -> Result<Option<Memory>, PostgresError> {
        let key = distill_key(Some(person_id), session_id);
        let mut unblock_quarantine = false;
        if !fresh {
            let active = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(
                     SELECT 1 FROM cairn_jobs WHERE kind = 'distill' AND job_key = $1 AND NOT succeeded
                 )",
            )
            .bind(&key)
            .fetch_one(&self.pool)
            .await?;
            if active {
                return Ok(None);
            }
            if self
                .distill_quarantine_blocks(person_id, session_id, &key)
                .await?
            {
                return Ok(None);
            }
            unblock_quarantine = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(
                     SELECT 1 FROM cairn_job_quarantine WHERE kind = 'distill' AND job_key = $1
                 )",
            )
            .bind(&key)
            .fetch_one(&self.pool)
            .await?;
        }
        let now = now_seconds();
        let mut tx = self.pool.begin().await?;
        enqueue_on(
            &mut tx,
            JobKind::Distill,
            &key,
            now,
            fresh || unblock_quarantine,
        )
        .await?;
        let claim = claim_on(&mut tx, JobKind::Distill.as_str(), Some(&key), 1, now).await?;
        tx.commit().await?;
        let Some(claim) = claim.into_iter().next() else {
            if fresh {
                return Err(PostgresError::Validation(
                    "distill job was not claimable".to_owned(),
                ));
            }
            return Ok(None);
        };
        self.execute_claimed_distill(&claim).await
    }

    async fn distill_quarantine_blocks(
        &self,
        person_id: &str,
        session_id: &str,
        key: &str,
    ) -> Result<bool, PostgresError> {
        let quarantined_at = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT max(quarantined_at) FROM cairn_job_quarantine WHERE kind = 'distill' AND job_key = $1",
        )
        .bind(key)
        .fetch_one(&self.pool)
        .await?;
        let Some(quarantined_at) = quarantined_at else {
            return Ok(false);
        };
        let newer = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
                 SELECT 1 FROM cairn_observations
                 WHERE person_id = $1 AND session_id = $2
                   AND captured_at > to_timestamp($3::double precision)
             )",
        )
        .bind(person_id)
        .bind(session_id)
        .bind(quarantined_at as f64)
        .fetch_one(&self.pool)
        .await?;
        Ok(!newer)
    }

    pub async fn execute_claimed_distill(
        &self,
        claim: &BackgroundJob,
    ) -> Result<Option<Memory>, PostgresError> {
        let claim_id = claim.claim_id.as_deref().unwrap_or("");
        let mut tx = self.pool.begin().await?;
        if !claim_is_held(&mut tx, &claim.job_id, claim_id).await? {
            tx.rollback().await?;
            return Err(PostgresError::Conflict(
                "distill claim is no longer held".to_owned(),
            ));
        }
        let Some((person_id, session_id)) = parse_distill_key(&claim.key) else {
            settle_on(
                &mut tx,
                &claim.job_id,
                claim_id,
                now_seconds(),
                JobEnd::Quarantine("distill job key is invalid".into()),
            )
            .await?;
            tx.commit().await?;
            return Err(PostgresError::Validation(
                "distill job key is invalid".to_owned(),
            ));
        };
        let Some(person_id) = person_id else {
            settle_on(
                &mut tx,
                &claim.job_id,
                claim_id,
                now_seconds(),
                JobEnd::Quarantine("distill job has no person".into()),
            )
            .await?;
            tx.commit().await?;
            return Err(PostgresError::Validation(
                "distill job has no person".to_owned(),
            ));
        };
        let covered_until = now_seconds();
        sqlx::query("SAVEPOINT distill_summary")
            .execute(&mut *tx)
            .await?;
        let result = self
            .finish_session_summary(&person_id, &session_id, &mut tx)
            .await;
        if result.is_err() {
            sqlx::query("ROLLBACK TO SAVEPOINT distill_summary")
                .execute(&mut *tx)
                .await?;
        }
        let end = match &result {
            Ok(_) => JobEnd::Succeeded,
            Err(error) => distill_job_end(&error.to_string()),
        };
        let when = if matches!(end, JobEnd::Quarantine(_)) {
            covered_until
        } else {
            now_seconds()
        };
        settle_on(&mut tx, &claim.job_id, claim_id, when, end).await?;
        tx.commit().await?;
        result
    }

    pub async fn reconcile_embeddings(&self, limit: usize, now: u64) -> Result<(), PostgresError> {
        let missing = self.embedding_jobs(limit).await?;
        let mut tx = self.pool.begin().await?;
        for memory in &missing {
            enqueue_on(&mut tx, JobKind::Embed, &memory.memory_id, now, false).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

async fn requeue_on(
    tx: &mut Transaction<'_, Postgres>,
    job_id: &str,
    now: u64,
) -> Result<(), PostgresError> {
    let row =
        sqlx::query("SELECT kind, job_key FROM cairn_job_quarantine WHERE job_id = $1 FOR UPDATE")
            .bind(job_id)
            .fetch_optional(&mut **tx)
            .await?;
    let Some(row) = row else {
        return Err(PostgresError::Validation(
            "unknown quarantined job".to_owned(),
        ));
    };
    let kind: String = row.try_get("kind")?;
    let key: String = row.try_get("job_key")?;
    let active = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1 FROM cairn_jobs WHERE kind = $1 AND job_key = $2 AND succeeded = false
         )",
    )
    .bind(&kind)
    .bind(&key)
    .fetch_one(&mut **tx)
    .await?;
    if active {
        return Err(PostgresError::Conflict(
            "a job for this key is already queued".to_owned(),
        ));
    }
    sqlx::query("DELETE FROM cairn_job_quarantine WHERE job_id = $1")
        .bind(job_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT INTO cairn_jobs
            (job_id, kind, job_key, attempts, not_before, claim_id, lease_expires_at, last_error, created_at, updated_at, finished_at, succeeded)
         VALUES ($1, $2, $3, 0, $4, NULL, NULL, NULL, $4, $4, NULL, false)",
    )
    .bind(random_token("job"))
    .bind(kind)
    .bind(key)
    .bind(stamp(now)?)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn enqueue_embed_tx(
    tx: &mut Transaction<'_, Postgres>,
    memory_id: &str,
    now: u64,
) -> Result<(), PostgresError> {
    enqueue_on(tx, JobKind::Embed, memory_id, now, true).await?;
    Ok(())
}
