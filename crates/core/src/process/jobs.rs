/// Lease length for a claimed job.
pub const JOB_LEASE_SECONDS: i64 = 120;
pub const JOB_ATTEMPT_CAP: i32 = 5;
/// Waits after failures 1 through 4. The fifth failure quarantines.
pub const JOB_BACKOFF_SECONDS: [i64; 4] = [30, 60, 120, 240];
pub const JOB_POLL_SECONDS: u64 = 2;
/// Finished jobs are deleted by the idle sweep after this long.
pub const JOB_DONE_RETENTION_SECONDS: i64 = 24 * 60 * 60;
/// A quarantined job is retried by the sweep this long after it was quarantined (no human requeue),
/// and once at the first start after a server upgrade.
pub const JOB_QUARANTINE_RETRY_SECONDS: i64 = 24 * 60 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Embed,
    Distill,
    Consolidate,
    Link,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Embed => "embed",
            Self::Distill => "distill",
            Self::Consolidate => "consolidate",
            Self::Link => "link",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobEnd {
    Succeeded,
    Retry(String),
    Quarantine(String),
}

/// What happens after failure number `attempts` (1-based): wait `Some(seconds)` and retry, or
/// `None` to quarantine.
pub fn backoff_after(attempts: i32) -> Option<i64> {
    if attempts >= JOB_ATTEMPT_CAP {
        return None;
    }
    JOB_BACKOFF_SECONDS
        .get((attempts.max(1) - 1) as usize)
        .copied()
}

pub fn distill_key(person_id: &str, session_id: &str) -> String {
    format!("{}:{person_id}{session_id}", person_id.len())
}

pub fn parse_distill_key(key: &str) -> Option<(String, String)> {
    let (len, rest) = key.split_once(':')?;
    let len: usize = len.parse().ok()?;
    let person = rest.get(..len)?;
    let session = rest.get(len..)?;
    (!person.is_empty() && !session.is_empty()).then(|| (person.to_owned(), session.to_owned()))
}

/// A session with nothing retained has nothing to distill: a normal close, not a failure.
pub fn is_empty_session(message: &str) -> bool {
    message
        .strip_prefix("validation error: ")
        .unwrap_or(message)
        .starts_with("session has no retained observations")
}

/// Bad vectors quarantine immediately; anything else retries with backoff.
pub fn embed_job_end(message: &str) -> JobEnd {
    if message.contains("batch length mismatch") || message.contains("finite normalized") {
        JobEnd::Quarantine(message.to_owned())
    } else {
        JobEnd::Retry(message.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_then_quarantine() {
        assert_eq!(backoff_after(1), Some(30));
        assert_eq!(backoff_after(4), Some(240));
        assert_eq!(backoff_after(5), None);
    }

    #[test]
    fn distill_key_round_trips_with_colons_in_ids() {
        let key = distill_key("person:1", "sess:2");
        assert_eq!(
            parse_distill_key(&key),
            Some(("person:1".into(), "sess:2".into()))
        );
        assert_eq!(parse_distill_key("3:ab"), None);
        assert!(is_empty_session(
            "validation error: session has no retained observations: s"
        ));
        assert!(!is_empty_session("session x has no retained observations"));
    }
}
