use cairn_core::{JOB_BACKOFF_SECONDS, JOB_LEASE_SECONDS, JobEnd, JobKind, embed_job_end};
use cairn_storage::PostgresStore;

fn url() -> String {
    std::env::var("CAIRN_TEST_DATABASE_URL")
        .expect("CAIRN_TEST_DATABASE_URL must point to a fresh PostgreSQL database")
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_jobs_lease_backoff_quarantine_and_exclusion() {
    let store = PostgresStore::connect(&url()).await.unwrap();
    let now = 1_700_000_000;
    store
        .enqueue_job(JobKind::Embed, "one", now, true)
        .await
        .unwrap();
    store
        .enqueue_job(JobKind::Embed, "two", now, true)
        .await
        .unwrap();
    let left = store.clone();
    let right = store.clone();
    let (first, second) = tokio::join!(
        left.poll_jobs(JobKind::Embed, 1, now),
        right.poll_jobs(JobKind::Embed, 1, now)
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_ne!(first[0].job_id, second[0].job_id);
    assert_ne!(first[0].claim_id, second[0].claim_id);
    let third = store.poll_jobs(JobKind::Embed, 1, now).await.unwrap();
    assert!(third.is_empty());

    let claim_id = first[0].claim_id.clone().unwrap();
    assert!(
        !store
            .settle_job(&first[0].job_id, "other", now, JobEnd::Succeeded)
            .await
            .unwrap()
    );
    assert!(
        store
            .settle_job(
                &first[0].job_id,
                &claim_id,
                now,
                JobEnd::Retry("database error: down".into())
            )
            .await
            .unwrap()
    );
    assert!(
        store
            .poll_jobs(JobKind::Embed, 8, now)
            .await
            .unwrap()
            .iter()
            .all(|job| job.job_id != first[0].job_id)
    );
    let retried = store
        .poll_jobs(JobKind::Embed, 8, now + JOB_BACKOFF_SECONDS[0])
        .await
        .unwrap();
    let retried = retried
        .into_iter()
        .find(|job| job.job_id == first[0].job_id)
        .unwrap();
    assert_eq!(retried.attempts, 1);
    assert!(
        store
            .settle_job(
                &retried.job_id,
                retried.claim_id.as_deref().unwrap(),
                now,
                JobEnd::Succeeded
            )
            .await
            .unwrap()
    );

    let expired = second[0].clone();
    assert!(
        store
            .poll_jobs(JobKind::Embed, 8, now + JOB_LEASE_SECONDS)
            .await
            .unwrap()
            .iter()
            .all(|job| job.job_id != expired.job_id)
    );
    let snapshot = store.job_snapshot(now + JOB_LEASE_SECONDS).await.unwrap();
    assert_eq!(snapshot.running, 0);
    assert!(snapshot.pending >= 1);

    store
        .enqueue_job(JobKind::Embed, "permanent", now, true)
        .await
        .unwrap();
    let bad = store
        .poll_jobs(JobKind::Embed, 8, now + JOB_BACKOFF_SECONDS[0])
        .await
        .unwrap()
        .into_iter()
        .find(|job| job.key == "permanent")
        .unwrap();
    assert!(
        store
            .settle_job(
                &bad.job_id,
                bad.claim_id.as_deref().unwrap(),
                now,
                embed_job_end("embedding batch length mismatch")
            )
            .await
            .unwrap()
    );
    let snapshot = store.job_snapshot(now).await.unwrap();
    let row = snapshot
        .quarantine
        .iter()
        .find(|row| row.job_id == bad.job_id)
        .unwrap();
    assert_eq!(row.attempts, 1);
    store
        .enqueue_job(JobKind::Embed, "permanent", now, true)
        .await
        .unwrap();
    assert!(store.requeue_job(&bad.job_id, now).await.is_err());
    let replacement = store
        .poll_jobs(JobKind::Embed, 8, now)
        .await
        .unwrap()
        .into_iter()
        .find(|job| job.key == "permanent")
        .unwrap();
    assert!(
        store
            .settle_job(
                &replacement.job_id,
                replacement.claim_id.as_deref().unwrap(),
                now,
                JobEnd::Succeeded
            )
            .await
            .unwrap()
    );
    store.requeue_job(&bad.job_id, now).await.unwrap();
    let snapshot = store.job_snapshot(now).await.unwrap();
    assert!(
        snapshot
            .quarantine
            .iter()
            .all(|row| row.job_id != bad.job_id)
    );
    assert!(snapshot.pending >= 1);
    let requeued = store.job_snapshot(now).await.unwrap().pending;
    assert!(requeued >= 1);
    let queued = store.poll_jobs(JobKind::Embed, 8, now).await.unwrap();
    let queued = queued
        .into_iter()
        .find(|job| job.key == "permanent")
        .unwrap();
    assert_eq!(queued.attempts, 0);
    store
        .settle_job(
            &queued.job_id,
            queued.claim_id.as_deref().unwrap(),
            now,
            JobEnd::Quarantine("conflict: stuck".into()),
        )
        .await
        .unwrap();
    store.discard_job(&queued.job_id).await.unwrap();
    assert!(store.discard_job(&queued.job_id).await.is_err());
    assert!(store.requeue_job(&queued.job_id, now).await.is_err());
}
