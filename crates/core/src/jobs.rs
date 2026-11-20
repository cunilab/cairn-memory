use crate::accounts::random_token;
use serde::{Deserialize, Serialize};

pub const JOB_LEASE_SECONDS: u64 = 120;
pub const JOB_ATTEMPT_CAP: u32 = 5;
/// Waits after failures 1 through 4. The fifth failure quarantines.
pub const JOB_BACKOFF_SECONDS: [u64; 4] = [30, 60, 120, 240];
pub const JOB_POLL_SECONDS: u64 = 2;
pub const JOB_SUCCEEDED_RETENTION_SECONDS: u64 = 24 * 60 * 60;
pub const EMBED_BATCH: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Embed,
    Distill,
    Similarity,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Embed => "embed",
            Self::Distill => "distill",
            Self::Similarity => "similarity",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobEnd {
    Succeeded,
    Retry(String),
    Quarantine(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobOpError {
    Missing,
    Busy,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackgroundJob {
    pub job_id: String,
    pub kind: String,
    pub key: String,
    pub attempts: u32,
    pub not_before: u64,
    pub claim_id: Option<String>,
    pub lease_expires_at: Option<u64>,
    pub last_error: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub finished_at: Option<u64>,
    pub succeeded: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuarantinedJob {
    pub job_id: String,
    pub kind: String,
    pub key: String,
    pub attempts: u32,
    pub last_error: String,
    pub created_at: u64,
    pub failed_at: u64,
    pub quarantined_at: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct JobSnapshot {
    pub pending: usize,
    pub running: usize,
    pub quarantined: usize,
    pub quarantine: Vec<QuarantinedJob>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobBook {
    pub jobs: Vec<BackgroundJob>,
    pub quarantine: Vec<QuarantinedJob>,
}

pub fn distill_key(person_id: Option<&str>, session_id: &str) -> String {
    let person = person_id.unwrap_or("");
    format!("{}:{person}{session_id}", person.len())
}

pub fn parse_distill_key(key: &str) -> Option<(Option<String>, String)> {
    let (len, rest) = key.split_once(':')?;
    let len: usize = len.parse().ok()?;
    let person = rest.get(..len)?;
    let session = rest.get(len..)?;
    if session.is_empty() {
        return None;
    }
    let person = if person.is_empty() {
        None
    } else {
        Some(person.to_owned())
    };
    Some((person, session.to_owned()))
}

pub fn job_label(kind: &str, key: &str) -> String {
    if kind == JobKind::Distill.as_str()
        && let Some((person, session)) = parse_distill_key(key)
    {
        return format!(
            "distill session {session} for {}",
            person.as_deref().unwrap_or("local")
        );
    }
    if kind == JobKind::Embed.as_str() {
        return format!("embed {key}");
    }
    if kind == JobKind::Similarity.as_str() {
        return "similarity scan".to_owned();
    }
    format!("{kind} {key}")
}

/// Embedding publish `false` is success at the call site. These strings quarantine immediately.
pub fn embed_job_end(message: &str) -> JobEnd {
    if message.contains("batch length mismatch") || message.contains("finite normalized") {
        JobEnd::Quarantine(message.to_owned())
    } else {
        JobEnd::Retry(message.to_owned())
    }
}

/// A session with nothing retained has nothing to distill. That is a normal close, not a failure.
/// The prefix is strict so a session id cannot pass for it; PostgreSQL wraps the core error.
pub fn is_empty_session(message: &str) -> bool {
    message
        .strip_prefix("core validation error: ")
        .unwrap_or(message)
        .starts_with("validation error: session has no retained observations")
}

/// A forgotten, already-current, or empty session is success. IO retries. Other distill errors
/// quarantine. The session id is the suffix of a validation error, so classification uses the
/// stable prefix.
pub fn distill_job_end(message: &str) -> JobEnd {
    if message.starts_with("validation error: session summary was forgotten or superseded")
        || is_empty_session(message)
    {
        JobEnd::Succeeded
    } else if message.starts_with("database error:")
        || message.starts_with("storage error:")
        || message.starts_with("JSON encoding error:")
        || message.starts_with("store format error:")
    {
        JobEnd::Retry(message.to_owned())
    } else {
        JobEnd::Quarantine(message.to_owned())
    }
}

fn row_is_active(jobs: &[BackgroundJob], kind: &str, key: &str) -> bool {
    jobs.iter()
        .any(|job| job.kind == kind && job.key == key && !job.succeeded)
}

fn live(job: &BackgroundJob, now: u64) -> bool {
    !job.succeeded && job.claim_id.is_some() && job.lease_expires_at.unwrap_or(0) > now
}

fn pending_job(kind: &str, key: &str, now: u64) -> BackgroundJob {
    BackgroundJob {
        job_id: random_token("job"),
        kind: kind.to_owned(),
        key: key.to_owned(),
        attempts: 0,
        not_before: now,
        claim_id: None,
        lease_expires_at: None,
        last_error: None,
        created_at: now,
        updated_at: now,
        finished_at: None,
        succeeded: false,
    }
}

impl JobBook {
    /// `fresh` replaces an older pending row for the key. A live lease is left in place and a new
    /// pending row is added behind it. `fresh == false` inserts only when no active row exists.
    pub fn enqueue(&mut self, kind: JobKind, key: &str, now: u64, fresh: bool) -> bool {
        let kind = kind.as_str();
        let same = |job: &BackgroundJob| job.kind == kind && job.key == key && !job.succeeded;
        if !fresh {
            if self.jobs.iter().any(same)
                || self
                    .quarantine
                    .iter()
                    .any(|row| row.kind == kind && row.key == key)
            {
                return false;
            }
            self.jobs.push(pending_job(kind, key, now));
            return true;
        }
        let before = self.jobs.len();
        self.jobs.retain(|job| !(same(job) && !live(job, now)));
        let removed = self.jobs.len() != before;
        if self
            .jobs
            .iter()
            .any(|job| same(job) && job.claim_id.is_none())
        {
            return removed;
        }
        self.jobs.push(pending_job(kind, key, now));
        true
    }

    pub fn maintain(&mut self, now: u64) -> bool {
        let before = self.jobs.len();
        self.jobs.retain(|job| {
            !(job.succeeded
                && now.saturating_sub(job.finished_at.unwrap_or(now))
                    >= JOB_SUCCEEDED_RETENTION_SECONDS)
        });
        let mut changed = self.jobs.len() != before;
        let expired = self
            .jobs
            .iter()
            .filter(|job| {
                !job.succeeded && job.claim_id.is_some() && job.lease_expires_at.unwrap_or(0) <= now
            })
            .map(|job| (job.job_id.clone(), job.claim_id.clone().unwrap_or_default()))
            .collect::<Vec<_>>();
        for (job_id, claim_id) in expired {
            changed |= self.fail(&job_id, &claim_id, "lease expired", false, now) == Some(true);
        }
        changed
    }

    pub fn claim(&mut self, kind: JobKind, limit: usize, now: u64) -> Vec<BackgroundJob> {
        let kind = kind.as_str();
        let mut indexes = self
            .jobs
            .iter()
            .enumerate()
            .filter(|(_, job)| {
                job.kind == kind
                    && !job.succeeded
                    && job.claim_id.is_none()
                    && job.not_before <= now
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        indexes.retain(|&index| {
            let kind = self.jobs[index].kind.clone();
            let key = self.jobs[index].key.clone();
            !self.live_key(&kind, &key, now)
        });
        indexes.sort_by(|&left, &right| {
            self.jobs[left]
                .not_before
                .cmp(&self.jobs[right].not_before)
                .then(self.jobs[left].job_id.cmp(&self.jobs[right].job_id))
        });
        indexes.truncate(limit);
        let mut claimed = Vec::with_capacity(indexes.len());
        for index in indexes {
            let job = &mut self.jobs[index];
            job.claim_id = Some(random_token("claim"));
            job.lease_expires_at = Some(now.saturating_add(JOB_LEASE_SECONDS));
            job.updated_at = now;
            claimed.push(job.clone());
        }
        claimed
    }

    pub fn claim_key(&mut self, kind: JobKind, key: &str, now: u64) -> Option<BackgroundJob> {
        let kind = kind.as_str();
        if self.live_key(kind, key, now) {
            return None;
        }
        let job = self.jobs.iter_mut().find(|job| {
            job.kind == kind
                && job.key == key
                && !job.succeeded
                && job.claim_id.is_none()
                && job.not_before <= now
        })?;
        job.claim_id = Some(random_token("claim"));
        job.lease_expires_at = Some(now.saturating_add(JOB_LEASE_SECONDS));
        job.updated_at = now;
        Some(job.clone())
    }

    pub fn settle(&mut self, job_id: &str, claim_id: &str, now: u64, end: JobEnd) -> bool {
        match end {
            JobEnd::Succeeded => self.complete(job_id, claim_id, now),
            JobEnd::Retry(error) => self
                .fail(job_id, claim_id, &error, false, now)
                .unwrap_or(false),
            JobEnd::Quarantine(error) => self
                .fail(job_id, claim_id, &error, true, now)
                .unwrap_or(false),
        }
    }

    fn complete(&mut self, job_id: &str, claim_id: &str, now: u64) -> bool {
        let Some(job) = self.jobs.iter_mut().find(|job| job.job_id == job_id) else {
            return false;
        };
        if job.succeeded || job.claim_id.as_deref() != Some(claim_id) {
            return false;
        }
        job.succeeded = true;
        job.claim_id = None;
        job.lease_expires_at = None;
        job.last_error = None;
        job.finished_at = Some(now);
        job.updated_at = now;
        true
    }

    /// `None` when the claim does not own the row. `Some` is whether the row changed.
    fn fail(
        &mut self,
        job_id: &str,
        claim_id: &str,
        error: &str,
        permanent: bool,
        now: u64,
    ) -> Option<bool> {
        let index = self.jobs.iter().position(|job| job.job_id == job_id)?;
        if self.jobs[index].succeeded || self.jobs[index].claim_id.as_deref() != Some(claim_id) {
            return None;
        }
        let mut job = self.jobs.remove(index);
        job.attempts = job.attempts.saturating_add(1);
        job.last_error = Some(error.to_owned());
        job.updated_at = now;
        if permanent || job.attempts >= JOB_ATTEMPT_CAP {
            self.quarantine.push(QuarantinedJob {
                job_id: job.job_id,
                kind: job.kind,
                key: job.key,
                attempts: job.attempts,
                last_error: error.to_owned(),
                created_at: job.created_at,
                failed_at: now,
                quarantined_at: now,
            });
            return Some(true);
        }
        let successor = self.jobs.iter().any(|other| {
            other.kind == job.kind
                && other.key == job.key
                && !other.succeeded
                && other.claim_id.is_none()
        });
        if successor {
            return Some(true);
        }
        job.not_before = now.saturating_add(JOB_BACKOFF_SECONDS[(job.attempts - 1) as usize]);
        job.claim_id = None;
        job.lease_expires_at = None;
        self.jobs.push(job);
        Some(true)
    }

    fn live_key(&self, kind: &str, key: &str, now: u64) -> bool {
        self.jobs
            .iter()
            .any(|job| job.kind == kind && job.key == key && live(job, now))
    }

    pub fn snapshot(&self, now: u64) -> JobSnapshot {
        let running = self.jobs.iter().filter(|job| live(job, now)).count();
        let pending = self
            .jobs
            .iter()
            .filter(|job| !job.succeeded && !live(job, now))
            .count();
        let mut quarantine = self.quarantine.clone();
        quarantine.sort_by(|left, right| {
            right
                .quarantined_at
                .cmp(&left.quarantined_at)
                .then(left.job_id.cmp(&right.job_id))
        });
        JobSnapshot {
            pending,
            running,
            quarantined: quarantine.len(),
            quarantine,
        }
    }

    pub fn requeue(&mut self, job_id: &str, now: u64) -> Result<(), JobOpError> {
        let index = self
            .quarantine
            .iter()
            .position(|row| row.job_id == job_id)
            .ok_or(JobOpError::Missing)?;
        let kind = self.quarantine[index].kind.clone();
        let key = self.quarantine[index].key.clone();
        if row_is_active(&self.jobs, &kind, &key) {
            return Err(JobOpError::Busy);
        }
        self.quarantine.remove(index);
        self.jobs.push(pending_job(&kind, &key, now));
        Ok(())
    }

    pub fn discard(&mut self, job_id: &str) -> Result<(), JobOpError> {
        let before = self.quarantine.len();
        self.quarantine.retain(|row| row.job_id != job_id);
        if self.quarantine.len() == before {
            Err(JobOpError::Missing)
        } else {
            Ok(())
        }
    }
}

impl crate::MemoryStore {
    pub(crate) fn with_jobs<T>(&mut self, change: impl FnOnce(&mut JobBook) -> T) -> T {
        let mut book = JobBook {
            jobs: std::mem::take(&mut self.state.jobs),
            quarantine: std::mem::take(&mut self.state.job_quarantine),
        };
        let result = change(&mut book);
        self.state.jobs = book.jobs;
        self.state.job_quarantine = book.quarantine;
        result
    }

    pub(crate) fn queue_changed_embeddings(&mut self) {
        use std::collections::HashMap;
        let now = crate::now_seconds();
        let previous = self
            .persisted
            .memories
            .iter()
            .map(|memory| (memory.memory_id.as_str(), memory))
            .collect::<HashMap<_, _>>();
        let changed = self
            .state
            .memories
            .iter()
            .filter(|memory| match previous.get(memory.memory_id.as_str()) {
                Some(before) => crate::embeddings::embedding_source_changed(before, memory),
                None => true,
            })
            .map(|memory| memory.memory_id.clone())
            .collect::<Vec<_>>();
        if changed.is_empty() {
            return;
        }
        self.with_jobs(|book| {
            for memory_id in &changed {
                book.enqueue(JobKind::Embed, memory_id, now, true);
            }
        });
    }

    pub fn reconcile_embeddings(
        &mut self,
        limit: usize,
        now: u64,
    ) -> Result<bool, crate::StoreError> {
        let missing = self
            .embedding_jobs(limit)
            .into_iter()
            .map(|memory| memory.memory_id)
            .collect::<Vec<_>>();
        let changed = self.with_jobs(|book| {
            let mut changed = false;
            for memory_id in &missing {
                changed |= book.enqueue(JobKind::Embed, memory_id, now, false);
            }
            changed
        });
        if changed {
            self.persist()?;
        }
        Ok(changed)
    }

    pub fn poll_jobs(
        &mut self,
        kind: JobKind,
        limit: usize,
        now: u64,
    ) -> Result<Vec<BackgroundJob>, crate::StoreError> {
        let maintained = self.with_jobs(|book| book.maintain(now));
        let reconciled = if kind == JobKind::Embed {
            let missing = self
                .embedding_jobs(limit)
                .into_iter()
                .map(|memory| memory.memory_id)
                .collect::<Vec<_>>();
            self.with_jobs(|book| {
                let mut changed = false;
                for memory_id in &missing {
                    changed |= book.enqueue(JobKind::Embed, memory_id, now, false);
                }
                changed
            })
        } else {
            false
        };
        let claimed = self.with_jobs(|book| book.claim(kind, limit, now));
        if maintained || reconciled || !claimed.is_empty() {
            self.persist()?;
        }
        Ok(claimed)
    }

    pub fn settle_job(
        &mut self,
        job_id: &str,
        claim_id: &str,
        now: u64,
        end: JobEnd,
    ) -> Result<bool, crate::StoreError> {
        let applied = self.with_jobs(|book| book.settle(job_id, claim_id, now, end));
        if applied {
            self.persist()?;
        }
        Ok(applied)
    }

    pub fn prepare_embed_work(
        &mut self,
        claims: Vec<BackgroundJob>,
        now: u64,
    ) -> Result<Vec<(BackgroundJob, crate::Memory)>, crate::StoreError> {
        let mut work = Vec::new();
        let mut applied = false;
        for claim in claims {
            let claim_id = claim.claim_id.clone().unwrap_or_default();
            let memory = self.recallable_memory(&claim.key);
            if let Some(memory) = memory.filter(|memory| !self.embedding_is_current(memory)) {
                work.push((claim, memory));
            } else {
                applied |= self.with_jobs(|book| {
                    book.settle(&claim.job_id, &claim_id, now, JobEnd::Succeeded)
                });
            }
        }
        if applied {
            self.persist()?;
        }
        Ok(work)
    }

    fn recallable_memory(&self, memory_id: &str) -> Option<crate::Memory> {
        let hidden = self.live_superseded_targets();
        self.state
            .memories
            .iter()
            .find(|memory| {
                memory.memory_id == memory_id && self.memory_is_recalled(memory, &hidden)
            })
            .cloned()
    }

    fn embedding_is_current(&self, memory: &crate::Memory) -> bool {
        self.state
            .embeddings
            .iter()
            .any(|embedding| embedding.matches(memory))
    }

    pub fn job_snapshot(&self, now: u64) -> JobSnapshot {
        JobBook {
            jobs: self.state.jobs.clone(),
            quarantine: self.state.job_quarantine.clone(),
        }
        .snapshot(now)
    }

    pub fn admin_jobs(&self, token: &str, now: u64) -> Result<JobSnapshot, crate::StoreError> {
        self.authenticate_admin(token)?;
        Ok(self.job_snapshot(now))
    }

    pub fn requeue_job(&mut self, job_id: &str, now: u64) -> Result<(), crate::StoreError> {
        self.mutate_quarantine(|book| book.requeue(job_id, now))
    }

    pub fn discard_job(&mut self, job_id: &str) -> Result<(), crate::StoreError> {
        self.mutate_quarantine(|book| book.discard(job_id))
    }

    fn mutate_quarantine(
        &mut self,
        change: impl FnOnce(&mut JobBook) -> Result<(), JobOpError>,
    ) -> Result<(), crate::StoreError> {
        let changed = self.with_jobs(change);
        match changed {
            Ok(()) => self.persist(),
            Err(JobOpError::Missing) => Err(crate::StoreError::Validation(
                "unknown quarantined job".to_owned(),
            )),
            Err(JobOpError::Busy) => Err(crate::StoreError::Conflict(
                "a job for this key is already queued".to_owned(),
            )),
        }
    }

    pub(crate) fn distill_is_idle_paused(&self, person_id: &str, session_id: &str) -> bool {
        let key = distill_key(Some(person_id), session_id);
        if row_is_active(&self.state.jobs, JobKind::Distill.as_str(), &key) {
            return true;
        }
        let Some(paused_at) = self
            .state
            .job_quarantine
            .iter()
            .filter(|row| row.kind == JobKind::Distill.as_str() && row.key == key)
            .map(|row| row.quarantined_at)
            .max()
        else {
            return false;
        };
        let newest = self
            .state
            .observations
            .iter()
            .filter(|observation| observation.session_id == session_id)
            .map(|observation| observation.captured_at.parse::<u64>().unwrap_or(0))
            .max()
            .unwrap_or(0);
        newest <= paused_at
    }
}

#[cfg(test)]
mod tests;
