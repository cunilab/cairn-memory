use super::*;
use crate::test_support::{observation_for, temp_store_path};
use crate::{MemoryStore, WorkContext};

fn book_with(kind: JobKind, key: &str, now: u64) -> (JobBook, BackgroundJob) {
    let mut book = JobBook::default();
    assert!(book.enqueue(kind, key, now, true));
    let claimed = book.claim(kind, 1, now);
    (book, claimed[0].clone())
}

#[test]
fn two_claims_never_take_the_same_job() {
    let mut book = JobBook::default();
    book.enqueue(JobKind::Embed, "one", 10, true);
    book.enqueue(JobKind::Embed, "two", 10, true);
    let first = book.claim(JobKind::Embed, 1, 10);
    let second = book.claim(JobKind::Embed, 1, 10);
    let third = book.claim(JobKind::Embed, 1, 10);
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert!(third.is_empty());
    assert_ne!(first[0].job_id, second[0].job_id);
    assert_ne!(first[0].claim_id, second[0].claim_id);
    assert!(!book.complete(&first[0].job_id, "other-claim", 10));
}

#[test]
fn expired_lease_counts_as_an_attempt_and_waits() {
    let (mut book, claimed) = book_with(JobKind::Embed, "memory", 1_000);
    let claim_id = claimed.claim_id.clone().unwrap();
    assert!(!book.maintain(1_000 + JOB_LEASE_SECONDS - 1));
    assert!(book.complete(&claimed.job_id, &claim_id, 1_000));
    let (mut book, claimed) = book_with(JobKind::Embed, "memory", 1_000);
    let claim_id = claimed.claim_id.unwrap();
    assert!(book.maintain(1_000 + JOB_LEASE_SECONDS));
    assert!(!book.complete(&claimed.job_id, &claim_id, 1_120));
    let snapshot = book.snapshot(1_120);
    assert_eq!(snapshot.running, 0);
    assert_eq!(snapshot.pending, 1);
    assert_eq!(snapshot.quarantined, 0);
    assert!(book.claim(JobKind::Embed, 1, 1_120).is_empty());
    let again = book.claim(JobKind::Embed, 1, 1_120 + JOB_BACKOFF_SECONDS[0]);
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].attempts, 1);
    assert_ne!(again[0].claim_id.as_deref(), Some(claim_id.as_str()));
}

#[test]
fn fifth_failure_quarantines_and_a_permanent_error_does_not_retry() {
    let (mut book, mut claimed) = book_with(JobKind::Distill, "session", 50);
    for attempt in 1..JOB_ATTEMPT_CAP {
        let claim_id = claimed.claim_id.clone().unwrap();
        assert!(book.settle(
            &claimed.job_id,
            &claim_id,
            50,
            JobEnd::Retry(format!("database error: blip {attempt}"))
        ));
        let job = book
            .jobs
            .iter()
            .find(|job| job.job_id == claimed.job_id)
            .unwrap();
        assert_eq!(job.attempts, attempt);
        assert_eq!(
            job.not_before,
            50 + JOB_BACKOFF_SECONDS[(attempt - 1) as usize]
        );
        claimed = book
            .claim_key(JobKind::Distill, "session", job.not_before)
            .unwrap();
    }
    let claim_id = claimed.claim_id.unwrap();
    assert!(book.settle(
        &claimed.job_id,
        &claim_id,
        90,
        JobEnd::Retry("database error: last".into())
    ));
    assert!(
        book.jobs
            .iter()
            .all(|job| job.succeeded || job.key != "session")
    );
    assert_eq!(book.quarantine.len(), 1);
    assert_eq!(book.quarantine[0].attempts, JOB_ATTEMPT_CAP);
    assert_eq!(JOB_BACKOFF_SECONDS[4], 480);

    let (mut book, claimed) = book_with(JobKind::Embed, "bad", 5);
    assert!(book.settle(
        &claimed.job_id,
        claimed.claim_id.as_deref().unwrap(),
        5,
        embed_job_end("embedding must be a finite normalized 384-dimensional vector")
    ));
    assert_eq!(book.quarantine.len(), 1);
    assert_eq!(book.quarantine[0].attempts, 1);
    assert!(book.claim(JobKind::Embed, 8, 5).is_empty());
}

#[test]
fn supersede_replaces_only_a_pending_row() {
    let mut book = JobBook::default();
    book.enqueue(JobKind::Embed, "memory", 1, true);
    let live = book.claim(JobKind::Embed, 1, 1).remove(0);
    book.enqueue(JobKind::Embed, "memory", 2, true);
    let pending = book
        .jobs
        .iter()
        .filter(|job| job.key == "memory" && !job.succeeded)
        .count();
    assert_eq!(pending, 2);
    assert!(
        book.jobs
            .iter()
            .any(|job| job.job_id == live.job_id && job.claim_id == live.claim_id)
    );
    let fresh = book
        .jobs
        .iter()
        .find(|job| job.key == "memory" && job.claim_id.is_none())
        .unwrap();
    assert_eq!(fresh.attempts, 0);
    assert_eq!(fresh.created_at, 2);
    book.enqueue(JobKind::Embed, "memory", 3, true);
    assert_eq!(
        book.jobs
            .iter()
            .filter(|job| job.key == "memory" && job.claim_id.is_none())
            .count(),
        1
    );
}

#[test]
fn requeue_and_discard_follow_the_quarantine_row() {
    let (mut book, claimed) = book_with(JobKind::Embed, "memory", 7);
    book.settle(
        &claimed.job_id,
        claimed.claim_id.as_deref().unwrap(),
        7,
        JobEnd::Quarantine("conflict: stuck".into()),
    );
    book.enqueue(JobKind::Embed, "memory", 8, true);
    assert_eq!(book.requeue(&claimed.job_id, 8), Err(JobOpError::Busy));
    assert_eq!(book.quarantine.len(), 1);
    book.jobs.retain(|job| job.succeeded);
    assert_eq!(book.requeue(&claimed.job_id, 9), Ok(()));
    assert!(book.quarantine.is_empty());
    let queued = book.jobs.iter().find(|job| !job.succeeded).unwrap();
    assert_eq!(queued.attempts, 0);
    assert_eq!(queued.not_before, 9);
    let job_id = queued.job_id.clone();
    let claim_id = book.claim(JobKind::Embed, 1, 9)[0]
        .claim_id
        .clone()
        .unwrap();
    book.settle(
        &job_id,
        &claim_id,
        9,
        JobEnd::Quarantine("validation error: unknown session".into()),
    );
    let quarantined = book.quarantine[0].job_id.clone();
    assert_eq!(book.discard(&quarantined), Ok(()));
    assert_eq!(book.discard(&quarantined), Err(JobOpError::Missing));
    assert_eq!(book.requeue(&quarantined, 10), Err(JobOpError::Missing));
}

#[test]
fn succeeded_rows_expire_after_one_day_and_counts_ignore_them() {
    let (mut book, claimed) = book_with(JobKind::Embed, "memory", 10);
    assert!(book.complete(&claimed.job_id, claimed.claim_id.as_deref().unwrap(), 10));
    let snapshot = book.snapshot(10);
    assert_eq!(
        (snapshot.pending, snapshot.running, snapshot.quarantined),
        (0, 0, 0)
    );
    assert!(!book.maintain(10 + JOB_SUCCEEDED_RETENTION_SECONDS - 1));
    assert_eq!(book.jobs.len(), 1);
    assert!(book.maintain(10 + JOB_SUCCEEDED_RETENTION_SECONDS));
    assert!(book.jobs.is_empty());
}

#[test]
fn distill_keys_round_trip_and_errors_are_classified() {
    let key = distill_key(Some("person"), "session/1");
    assert_eq!(
        parse_distill_key(&key),
        Some((Some("person".to_owned()), "session/1".to_owned()))
    );
    assert_eq!(
        parse_distill_key(&distill_key(None, "local")),
        Some((None, "local".to_owned()))
    );
    assert!(matches!(
        distill_job_end("validation error: session summary was forgotten or superseded: memory-1"),
        JobEnd::Succeeded
    ));
    assert!(matches!(
        distill_job_end("database error: connection reset"),
        JobEnd::Retry(_)
    ));
    assert!(matches!(
        distill_job_end("validation error: unknown session: missing"),
        JobEnd::Quarantine(_)
    ));
    assert!(matches!(
        embed_job_end("embedding inference failed"),
        JobEnd::Retry(_)
    ));
    assert!(matches!(
        embed_job_end("embedding batch length mismatch"),
        JobEnd::Quarantine(_)
    ));
}

#[test]
fn file_jobs_survive_a_crash_and_session_end_still_returns_the_summary() {
    let path = temp_store_path("jobs-file");
    let mut store = MemoryStore::open(&path).unwrap();
    let memory = store
        .remember("A durable note", vec![], WorkContext::default())
        .unwrap();
    let now = crate::now_seconds();
    let claimed = store.poll_jobs(JobKind::Embed, 1, now).unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].key, memory.memory_id);
    let job_id = claimed[0].job_id.clone();
    let claim_id = claimed[0].claim_id.clone().unwrap();
    drop(store);
    let mut store = MemoryStore::open(&path).unwrap();
    assert!(store.poll_jobs(JobKind::Embed, 1, now).unwrap().is_empty());
    assert!(
        !store
            .settle_job(&job_id, "other", now, JobEnd::Succeeded)
            .unwrap()
    );
    assert!(
        store
            .settle_job(
                &job_id,
                &claim_id,
                now,
                JobEnd::Retry("storage error: disk".into())
            )
            .unwrap()
    );
    store.state.memories[0].access_count += 1;
    store.persist().unwrap();
    let pending = store
        .state
        .jobs
        .iter()
        .find(|job| job.job_id == job_id)
        .unwrap();
    assert_eq!(pending.attempts, 1);
    assert_eq!(pending.not_before, now + JOB_BACKOFF_SECONDS[0]);
    store.state.memories[0].fields.title = "Changed title".into();
    store.persist().unwrap();
    let replaced = store
        .state
        .jobs
        .iter()
        .filter(|job| job.kind == "embed" && !job.succeeded)
        .collect::<Vec<_>>();
    assert_eq!(replaced.len(), 1);
    assert_eq!(replaced[0].attempts, 0);
    assert_ne!(replaced[0].job_id, job_id);

    store
        .start_session("sess-job", WorkContext::default())
        .unwrap();
    store
        .capture_observation(observation_for(
            "sess-job",
            "evt-job",
            "Decision: keep the summary in the response.",
        ))
        .unwrap();
    let summary = store.end_session("sess-job").unwrap();
    assert!(summary.content.contains("Decision"));
    assert!(
        store
            .state
            .jobs
            .iter()
            .any(|job| job.kind == "distill" && job.succeeded)
    );
    let before = store.state.job_quarantine.len();
    let error = store.end_session("missing-session").unwrap_err();
    assert!(error.to_string().contains("unknown session"));
    assert_eq!(store.state.job_quarantine.len(), before + 1);
    let _ = std::fs::remove_file(path);
}

#[test]
fn an_empty_session_is_quarantined_and_a_later_observation_can_run() {
    let path = temp_store_path("jobs-empty");
    let mut store = MemoryStore::open(&path).unwrap();
    store
        .start_session("sess-empty", WorkContext::default())
        .unwrap();
    let error = store.end_session("sess-empty").unwrap_err();
    assert!(error.to_string().contains("no retained observations"));
    assert_eq!(store.state.job_quarantine.len(), 1);
    assert!(store.state.job_quarantine[0].attempts >= 1);
    let _ = std::fs::remove_file(path);
}
