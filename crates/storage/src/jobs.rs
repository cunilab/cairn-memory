use super::*;
use cairn_core::{
    BackgroundJob, JobBook, JobEnd, JobKind, JobOpError, JobSnapshot, QuarantinedJob,
    distill_job_end, distill_key, now_seconds,
};
use sqlx::{Postgres, Transaction};

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

async fn load_book(tx: &mut Transaction<'_, Postgres>) -> Result<JobBook, PostgresError> {
    let jobs = sqlx::query(
        "SELECT job_id, kind, job_key, attempts, not_before, claim_id, lease_expires_at, last_error, created_at, updated_at, finished_at, succeeded FROM cairn_jobs",
    )
    .fetch_all(&mut **tx)
    .await?;
    let quarantine = sqlx::query(
        "SELECT job_id, kind, job_key, attempts, last_error, created_at, failed_at, quarantined_at FROM cairn_job_quarantine",
    )
    .fetch_all(&mut **tx)
    .await?;
    Ok(JobBook {
        jobs: jobs
            .iter()
            .map(job_from_row)
            .collect::<Result<Vec<_>, _>>()?,
        quarantine: quarantine
            .iter()
            .map(quarantine_from_row)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

async fn save_book(
    tx: &mut Transaction<'_, Postgres>,
    book: &JobBook,
) -> Result<(), PostgresError> {
    sqlx::query("DELETE FROM cairn_job_quarantine")
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM cairn_jobs")
        .execute(&mut **tx)
        .await?;
    for job in &book.jobs {
        sqlx::query(
            "INSERT INTO cairn_jobs (job_id, kind, job_key, attempts, not_before, claim_id, lease_expires_at, last_error, created_at, updated_at, finished_at, succeeded)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
        )
        .bind(&job.job_id)
        .bind(&job.kind)
        .bind(&job.key)
        .bind(i32::try_from(job.attempts).unwrap_or(i32::MAX))
        .bind(stamp(job.not_before)?)
        .bind(&job.claim_id)
        .bind(job.lease_expires_at.map(stamp).transpose()?)
        .bind(&job.last_error)
        .bind(stamp(job.created_at)?)
        .bind(stamp(job.updated_at)?)
        .bind(job.finished_at.map(stamp).transpose()?)
        .bind(job.succeeded)
        .execute(&mut **tx)
        .await?;
    }
    for row in &book.quarantine {
        sqlx::query(
            "INSERT INTO cairn_job_quarantine (job_id, kind, job_key, attempts, last_error, created_at, failed_at, quarantined_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(&row.job_id)
        .bind(&row.kind)
        .bind(&row.key)
        .bind(i32::try_from(row.attempts).unwrap_or(i32::MAX))
        .bind(&row.last_error)
        .bind(stamp(row.created_at)?)
        .bind(stamp(row.failed_at)?)
        .bind(stamp(row.quarantined_at)?)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn lock_book(tx: &mut Transaction<'_, Postgres>) -> Result<(), PostgresError> {
    sqlx::query("LOCK TABLE cairn_jobs, cairn_job_quarantine IN EXCLUSIVE MODE")
        .execute(&mut **tx)
        .await?;
    Ok(())
}

impl PostgresStore {
    async fn with_book<T>(
        &self,
        change: impl FnOnce(&mut JobBook) -> (T, bool),
    ) -> Result<T, PostgresError> {
        let mut tx = self.pool.begin().await?;
        lock_book(&mut tx).await?;
        let mut book = load_book(&mut tx).await?;
        let (result, dirty) = change(&mut book);
        if dirty {
            save_book(&mut tx, &book).await?;
        }
        tx.commit().await?;
        Ok(result)
    }

    pub async fn enqueue_job(
        &self,
        kind: JobKind,
        key: &str,
        now: u64,
        fresh: bool,
    ) -> Result<bool, PostgresError> {
        self.with_book(|book| {
            let inserted = book.enqueue(kind, key, now, fresh);
            (inserted, inserted)
        })
        .await
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
        self.with_book(|book| {
            let maintained = book.maintain(now);
            let reconciled = missing
                .iter()
                .any(|memory| book.enqueue(JobKind::Embed, &memory.memory_id, now, false));
            let claimed = book.claim(kind, limit, now);
            let dirty = maintained || reconciled || !claimed.is_empty();
            (claimed, dirty)
        })
        .await
    }

    pub async fn settle_job(
        &self,
        job_id: &str,
        claim_id: &str,
        now: u64,
        end: JobEnd,
    ) -> Result<bool, PostgresError> {
        self.with_book(|book| {
            let applied = book.settle(job_id, claim_id, now, end);
            (applied, applied)
        })
        .await
    }

    pub async fn job_snapshot(&self, now: u64) -> Result<JobSnapshot, PostgresError> {
        let mut tx = self.pool.begin().await?;
        let book = load_book(&mut tx).await?;
        tx.commit().await?;
        Ok(book.snapshot(now))
    }

    pub async fn admin_jobs(&self, token: &str, now: u64) -> Result<JobSnapshot, PostgresError> {
        self.authenticate_admin(token).await?;
        self.job_snapshot(now).await
    }

    pub async fn requeue_job(&self, job_id: &str, now: u64) -> Result<(), PostgresError> {
        self.mutate_quarantine(|book| book.requeue(job_id, now))
            .await
    }

    pub async fn discard_job(&self, job_id: &str) -> Result<(), PostgresError> {
        self.mutate_quarantine(|book| book.discard(job_id)).await
    }

    async fn mutate_quarantine(
        &self,
        change: impl FnOnce(&mut JobBook) -> Result<(), JobOpError>,
    ) -> Result<(), PostgresError> {
        self.with_book(|book| match change(book) {
            Ok(()) => (Ok(()), true),
            Err(error) => (Err(error), false),
        })
        .await?
        .map_err(|error| match error {
            JobOpError::Missing => PostgresError::Validation("unknown quarantined job".to_owned()),
            JobOpError::Busy => {
                PostgresError::Conflict("a job for this key is already queued".to_owned())
            }
        })
    }

    pub(super) async fn run_distill(
        &self,
        person_id: &str,
        session_id: &str,
        fresh: bool,
    ) -> Result<Option<Memory>, PostgresError> {
        let key = distill_key(Some(person_id), session_id);
        if !fresh
            && sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM cairn_jobs WHERE kind = 'distill' AND job_key = $1 AND NOT succeeded)",
            )
            .bind(&key)
            .fetch_one(&self.pool)
            .await?
        {
            return Ok(None);
        }
        let now = now_seconds();
        let claim = self
            .with_book(|book| {
                let inserted = book.enqueue(JobKind::Distill, &key, now, fresh);
                let claim = book.claim_key(JobKind::Distill, &key, now);
                let dirty = inserted || claim.is_some();
                (claim, dirty)
            })
            .await?;
        let Some(claim) = claim else {
            if fresh {
                return Err(PostgresError::Validation(
                    "distill job was not claimable".to_owned(),
                ));
            }
            return Ok(None);
        };
        self.execute_claimed_distill(&claim).await
    }

    pub async fn execute_claimed_distill(
        &self,
        claim: &BackgroundJob,
    ) -> Result<Option<Memory>, PostgresError> {
        let Some((person_id, session_id)) = cairn_core::parse_distill_key(&claim.key) else {
            self.settle_job(
                &claim.job_id,
                claim.claim_id.as_deref().unwrap_or(""),
                now_seconds(),
                JobEnd::Quarantine("distill job key is invalid".into()),
            )
            .await?;
            return Err(PostgresError::Validation(
                "distill job key is invalid".to_owned(),
            ));
        };
        let Some(person_id) = person_id else {
            self.settle_job(
                &claim.job_id,
                claim.claim_id.as_deref().unwrap_or(""),
                now_seconds(),
                JobEnd::Quarantine("distill job has no person".into()),
            )
            .await?;
            return Err(PostgresError::Validation(
                "distill job has no person".to_owned(),
            ));
        };
        let result = self.finish_session_summary(&person_id, &session_id).await;
        let end = match &result {
            Ok(_) => JobEnd::Succeeded,
            Err(error) => distill_job_end(&error.to_string()),
        };
        let quarantined = matches!(end, JobEnd::Quarantine(_));
        self.settle_job(
            &claim.job_id,
            claim.claim_id.as_deref().unwrap_or(""),
            now_seconds(),
            end,
        )
        .await?;
        if quarantined {
            sqlx::query(
                "UPDATE cairn_sessions SET pending_since = NULL WHERE person_id = $1 AND session_id = $2",
            )
            .bind(&person_id)
            .bind(&session_id)
            .execute(&self.pool)
            .await?;
        }
        result
    }

    pub async fn reconcile_embeddings(&self, limit: usize, now: u64) -> Result<(), PostgresError> {
        let missing = self.embedding_jobs(limit).await?;
        self.with_book(|book| {
            let changed = missing
                .iter()
                .any(|memory| book.enqueue(JobKind::Embed, &memory.memory_id, now, false));
            ((), changed)
        })
        .await
    }
}

pub(super) async fn enqueue_embed_tx(
    tx: &mut Transaction<'_, Postgres>,
    memory_id: &str,
    now: u64,
) -> Result<(), PostgresError> {
    lock_book(tx).await?;
    let mut book = load_book(tx).await?;
    if book.enqueue(JobKind::Embed, memory_id, now, true) {
        save_book(tx, &book).await?;
    }
    Ok(())
}
