use crate::account::audit_tx;
use crate::{Result, Store};
use cairn_core::id::new_id;
use cairn_core::process::jobs::{
    JOB_ATTEMPT_CAP, JOB_LEASE_SECONDS, JOB_QUARANTINE_RETRY_SECONDS, JobEnd, JobKind,
    backoff_after, parse_distill_key,
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
        let mut tx = self.pool.begin().await?;
        let lost = sqlx::query(
            "UPDATE jobs SET status = 'quarantined', claim_id = NULL, lease_expires_at = NULL,
                    last_error = 'lease expired too many times', updated_at = now(), finished_at = now()
              WHERE kind = $1 AND status = 'running' AND lease_expires_at <= now() AND attempts >= $2
              RETURNING kind, job_key, attempts, last_error",
        )
        .bind(kind.as_str())
        .bind(JOB_ATTEMPT_CAP)
        .fetch_all(&mut *tx)
        .await?;
        for row in &lost {
            note_quarantine(&mut tx, row).await?;
        }
        tx.commit().await?;
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

    /// No human requeue: a job quarantined for `JOB_QUARANTINE_RETRY_SECONDS` goes back to pending
    /// with one attempt left, so a poison job costs one try a day while a fixed one heals itself.
    /// Returns the jobs retried.
    pub async fn retry_quarantined(&self) -> Result<u64> {
        self.retry_quarantined_older_than(JOB_QUARANTINE_RETRY_SECONDS)
            .await
    }

    /// The first start of a new server version retries every quarantined job once, whatever its
    /// age: the upgrade may be what fixes it. The version is remembered, so a restart does not.
    pub async fn retry_quarantined_after_upgrade(&self, version: &str) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO server_state (key, value) VALUES ('version', $1) ON CONFLICT DO NOTHING",
        )
        .bind(version)
        .execute(&mut *tx)
        .await?;
        let known: String =
            sqlx::query_scalar("SELECT value FROM server_state WHERE key = 'version' FOR UPDATE")
                .fetch_one(&mut *tx)
                .await?;
        if known == version {
            tx.commit().await?;
            return Ok(0);
        }
        sqlx::query("UPDATE server_state SET value = $1 WHERE key = 'version'")
            .bind(version)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.retry_quarantined_older_than(0).await
    }

    /// A quarantined job whose key already has a pending job, or a newer quarantined row, is
    /// dropped: only the newest row per key is revived, so `jobs_one_pending` cannot fail the sweep.
    async fn retry_quarantined_older_than(&self, seconds: i64) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(&format!(
            "DELETE FROM jobs q WHERE q.status = 'quarantined'
                AND q.finished_at <= now() - make_interval(secs => {seconds})
                AND (EXISTS (SELECT 1 FROM jobs p WHERE p.kind = q.kind AND p.job_key = q.job_key AND p.status = 'pending')
                     OR EXISTS (SELECT 1 FROM jobs n WHERE n.kind = q.kind AND n.job_key = q.job_key AND n.status = 'quarantined'
                                   AND (n.finished_at, n.job_id) > (q.finished_at, q.job_id)))"
        ))
        .execute(&mut *tx)
        .await?;
        let done = sqlx::query(&format!(
            "UPDATE jobs SET status = 'pending', attempts = {}, not_before = now(), claim_id = NULL,
                    lease_expires_at = NULL, finished_at = NULL, updated_at = now()
              WHERE status = 'quarantined' AND finished_at <= now() - make_interval(secs => {seconds})",
            JOB_ATTEMPT_CAP - 1
        ))
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(done.rows_affected())
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
    let row = sqlx::query("UPDATE jobs SET status = 'quarantined', claim_id = NULL, lease_expires_at = NULL, last_error = $2, updated_at = now(), finished_at = now() WHERE job_id = $1 RETURNING kind, job_key, attempts, last_error")
        .bind(job_id)
        .bind(error)
        .fetch_one(&mut *tx)
        .await?;
    note_quarantine(tx, &row).await
}

/// The person whose job it is sees "a job stopped" in Activity. Jobs of promoted copies belong to
/// no person and are not reported.
async fn note_quarantine(tx: &mut PgConnection, job: &PgRow) -> Result<()> {
    let (kind, key): (String, String) = (job.try_get("kind")?, job.try_get("job_key")?);
    let owner: Option<String> = if kind == JobKind::Distill.as_str() {
        parse_distill_key(&key).map(|(person, _)| person)
    } else {
        sqlx::query_scalar("SELECT owner_id FROM memories WHERE memory_id = $1")
            .bind(&key)
            .fetch_optional(&mut *tx)
            .await?
    };
    let Some(owner) = owner.filter(|o| o != crate::consolidate::SYSTEM_OWNER) else {
        return Ok(());
    };
    let error: Option<String> = job.try_get("last_error")?;
    let error: String = error.unwrap_or_default().chars().take(300).collect();
    audit_tx(
        tx,
        &owner,
        None,
        "job_quarantined",
        Some(&key),
        serde_json::json!({"kind": kind, "attempts": job.try_get::<i32, _>("attempts")?, "error": error}),
    )
    .await
}
