use crate::{Result, Store, StoreError};
use cairn_core::id::new_id;
use cairn_core::process::jobs::{
    JOB_ATTEMPT_CAP, JOB_LEASE_SECONDS, JobEnd, JobKind, backoff_after,
};
use serde::Serialize;
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Job {
    pub job_id: String,
    pub kind: String,
    pub key: String,
    pub status: String,
    pub attempts: i32,
    pub claim_id: Option<String>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct JobSnapshot {
    pub pending: i64,
    pub running: i64,
    pub quarantined: i64,
    /// Quarantined jobs, newest first.
    pub quarantine: Vec<Job>,
}

const JOB_COLUMNS: &str = "job_id, kind, job_key, status, attempts, claim_id, last_error,
    EXTRACT(EPOCH FROM created_at)::bigint AS created_at, EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at";

fn job_from_row(row: &PgRow) -> Result<Job> {
    Ok(Job {
        job_id: row.try_get("job_id")?,
        kind: row.try_get("kind")?,
        key: row.try_get("job_key")?,
        status: row.try_get("status")?,
        attempts: row.try_get("attempts")?,
        claim_id: row.try_get("claim_id")?,
        last_error: row.try_get("last_error")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// One pending job per (kind, key). True when a new row was added.
pub(crate) async fn enqueue_tx(tx: &mut PgConnection, kind: JobKind, key: &str) -> Result<bool> {
    let done = sqlx::query(
        "INSERT INTO jobs (job_id, kind, job_key) VALUES ($1, $2, $3)
         ON CONFLICT (kind, job_key) WHERE status = 'pending' DO NOTHING",
    )
    .bind(new_id("job"))
    .bind(kind.as_str())
    .bind(key)
    .execute(tx)
    .await?;
    Ok(done.rows_affected() == 1)
}

impl Store {
    pub async fn enqueue_job(&self, kind: JobKind, key: &str) -> Result<bool> {
        let mut conn = self.pool.acquire().await?;
        enqueue_tx(&mut conn, kind, key).await
    }

    /// Claims up to `limit` due jobs of a kind with a lease. A job whose lease ran out is
    /// claimable again, so a crashed worker's job comes back. Claiming counts an attempt.
    pub async fn claim_jobs(&self, kind: JobKind, limit: i64) -> Result<Vec<Job>> {
        // A job that keeps losing its lease is a poison job: quarantine it instead of looping.
        sqlx::query(
            "UPDATE jobs SET status = 'quarantined', claim_id = NULL, lease_expires_at = NULL,
                    last_error = 'lease expired too many times', updated_at = now(), finished_at = now()
              WHERE kind = $1 AND status = 'running' AND lease_expires_at <= now() AND attempts >= $2",
        )
        .bind(kind.as_str())
        .bind(JOB_ATTEMPT_CAP)
        .execute(&self.pool)
        .await?;
        let rows = sqlx::query(&format!(
            "UPDATE jobs SET status = 'running', claim_id = $2, attempts = attempts + 1, updated_at = now(),
                    lease_expires_at = now() + make_interval(secs => {JOB_LEASE_SECONDS})
              WHERE job_id IN (
                    SELECT job_id FROM jobs
                     WHERE kind = $1 AND ((status = 'pending' AND not_before <= now())
                                       OR (status = 'running' AND lease_expires_at <= now()))
                     ORDER BY not_before, job_id LIMIT $3 FOR UPDATE SKIP LOCKED)
              RETURNING {JOB_COLUMNS}"
        ))
        .bind(kind.as_str())
        .bind(new_id("claim"))
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(job_from_row).collect()
    }

    /// Ends a claimed job. A stale claim (another worker took over) changes nothing: false.
    /// `Retry` waits out the backoff, and the fifth failure quarantines.
    pub async fn settle_job(&self, job_id: &str, claim_id: &str, end: JobEnd) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT kind, job_key, attempts FROM jobs WHERE job_id = $1 AND claim_id = $2 AND status = 'running' FOR UPDATE")
            .bind(job_id)
            .bind(claim_id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            return Ok(false);
        };
        let attempts: i32 = row.try_get("attempts")?;
        match end {
            JobEnd::Succeeded => {
                sqlx::query("UPDATE jobs SET status = 'done', claim_id = NULL, lease_expires_at = NULL, last_error = NULL, updated_at = now(), finished_at = now() WHERE job_id = $1")
                    .bind(job_id)
                    .execute(&mut *tx)
                    .await?;
            }
            JobEnd::Quarantine(error) => quarantine(&mut tx, job_id, &error).await?,
            JobEnd::Retry(error) => match backoff_after(attempts) {
                None => quarantine(&mut tx, job_id, &error).await?,
                Some(wait) => {
                    // Back to pending unless a newer pending row for the key already exists.
                    let kind: String = row.try_get("kind")?;
                    let key: String = row.try_get("job_key")?;
                    let waiting: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM jobs WHERE kind = $1 AND job_key = $2 AND status = 'pending')")
                        .bind(&kind)
                        .bind(&key)
                        .fetch_one(&mut *tx)
                        .await?;
                    if waiting {
                        sqlx::query("UPDATE jobs SET status = 'done', claim_id = NULL, lease_expires_at = NULL, last_error = $2, updated_at = now(), finished_at = now() WHERE job_id = $1")
                            .bind(job_id)
                            .bind(&error)
                            .execute(&mut *tx)
                            .await?;
                    } else {
                        sqlx::query("UPDATE jobs SET status = 'pending', claim_id = NULL, lease_expires_at = NULL, last_error = $2, not_before = now() + make_interval(secs => $3), updated_at = now() WHERE job_id = $1")
                            .bind(job_id)
                            .bind(&error)
                            .bind(wait as f64)
                            .execute(&mut *tx)
                            .await?;
                    }
                }
            },
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Admin: a quarantined job goes back to pending with a fresh attempt count. Busy (409) when
    /// the same job is already pending.
    pub async fn requeue_job(&self, job_id: &str) -> Result<()> {
        let done = sqlx::query(
            "UPDATE jobs SET status = 'pending', attempts = 0, not_before = now(), claim_id = NULL, lease_expires_at = NULL, updated_at = now()
              WHERE job_id = $1 AND status = 'quarantined'",
        )
        .bind(job_id)
        .execute(&self.pool)
        .await;
        match done {
            Ok(done) if done.rows_affected() == 1 => Ok(()),
            Ok(_) => Err(StoreError::NotFound("quarantined job".into())),
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                Err(StoreError::Conflict("job is already pending".into()))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Admin: drops a quarantined job.
    pub async fn discard_job(&self, job_id: &str) -> Result<()> {
        let done = sqlx::query("DELETE FROM jobs WHERE job_id = $1 AND status = 'quarantined'")
            .bind(job_id)
            .execute(&self.pool)
            .await?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound("quarantined job".into()));
        }
        Ok(())
    }

    /// Pending, running and quarantined jobs that belong to one person: embed and consolidate jobs
    /// of their own memories, and their distill jobs.
    pub async fn person_job_counts(&self, person_id: &str) -> Result<(i64, i64, i64)> {
        let counts = sqlx::query(
            "SELECT count(*) FILTER (WHERE j.status = 'pending') AS pending,
                    count(*) FILTER (WHERE j.status = 'running') AS running,
                    count(*) FILTER (WHERE j.status = 'quarantined') AS quarantined
               FROM jobs j
              WHERE (j.kind = 'distill' AND starts_with(j.job_key, length($1)::text || ':' || $1))
                 OR (j.kind <> 'distill' AND j.job_key IN (SELECT memory_id FROM memories WHERE owner_id = $1))",
        )
        .bind(person_id)
        .fetch_one(&self.pool)
        .await?;
        Ok((
            counts.try_get("pending")?,
            counts.try_get("running")?,
            counts.try_get("quarantined")?,
        ))
    }

    pub async fn job_snapshot(&self) -> Result<JobSnapshot> {
        let counts = sqlx::query(
            "SELECT count(*) FILTER (WHERE status = 'pending') AS pending,
                    count(*) FILTER (WHERE status = 'running') AS running,
                    count(*) FILTER (WHERE status = 'quarantined') AS quarantined FROM jobs",
        )
        .fetch_one(&self.pool)
        .await?;
        let rows = sqlx::query(&format!(
            "SELECT {JOB_COLUMNS} FROM jobs WHERE status = 'quarantined' ORDER BY updated_at DESC, job_id LIMIT 100"
        ))
        .fetch_all(&self.pool)
        .await?;
        Ok(JobSnapshot {
            pending: counts.try_get("pending")?,
            running: counts.try_get("running")?,
            quarantined: counts.try_get("quarantined")?,
            quarantine: rows.iter().map(job_from_row).collect::<Result<_>>()?,
        })
    }
}

async fn quarantine(tx: &mut PgConnection, job_id: &str, error: &str) -> Result<()> {
    sqlx::query("UPDATE jobs SET status = 'quarantined', claim_id = NULL, lease_expires_at = NULL, last_error = $2, updated_at = now(), finished_at = now() WHERE job_id = $1")
        .bind(job_id)
        .bind(error)
        .execute(tx)
        .await?;
    Ok(())
}
