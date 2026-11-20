use cairn_core::process::jobs::{JOB_ATTEMPT_CAP, JobEnd, JobKind};
use cairn_store::StoreError;
use cairn_store::testing::TestDb;

async fn make_due(db: &TestDb) {
    sqlx::query(
        "UPDATE jobs SET not_before = now() - interval '1 second' WHERE status = 'pending'",
    )
    .execute(db.store.pool())
    .await
    .unwrap();
}

#[tokio::test]
async fn one_pending_job_per_key_and_claims_do_not_overlap() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    assert!(store.enqueue_job(JobKind::Embed, "one").await.unwrap());
    assert!(
        !store.enqueue_job(JobKind::Embed, "one").await.unwrap(),
        "same key stays one job"
    );
    assert!(store.enqueue_job(JobKind::Embed, "two").await.unwrap());
    assert!(
        store
            .enqueue_job(JobKind::Consolidate, "one")
            .await
            .unwrap(),
        "kinds are separate"
    );

    let (left, right) = tokio::join!(
        store.claim_jobs(JobKind::Embed, 1),
        store.claim_jobs(JobKind::Embed, 1)
    );
    let (left, right) = (left.unwrap(), right.unwrap());
    assert_eq!((left.len(), right.len()), (1, 1));
    assert_ne!(left[0].job_id, right[0].job_id);
    assert_ne!(left[0].claim_id, right[0].claim_id);
    assert!(
        store
            .claim_jobs(JobKind::Embed, 5)
            .await
            .unwrap()
            .is_empty(),
        "leased jobs are not claimable"
    );

    let claim = left[0].claim_id.clone().unwrap();
    assert!(
        !store
            .settle_job(&left[0].job_id, "other-claim", JobEnd::Succeeded)
            .await
            .unwrap(),
        "stale claim"
    );
    assert!(
        store
            .settle_job(&left[0].job_id, &claim, JobEnd::Succeeded)
            .await
            .unwrap()
    );
    assert!(
        !store
            .settle_job(&left[0].job_id, &claim, JobEnd::Succeeded)
            .await
            .unwrap(),
        "settled once"
    );
    // Done jobs free the key.
    assert!(
        store
            .enqueue_job(JobKind::Embed, &left[0].key)
            .await
            .unwrap()
    );
    db.finish().await;
}

#[tokio::test]
async fn lease_expiry_backoff_quarantine_and_requeue() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    store.enqueue_job(JobKind::Embed, "k").await.unwrap();

    // A crashed worker: the lease runs out and the job comes back.
    let first = store.claim_jobs(JobKind::Embed, 1).await.unwrap().remove(0);
    sqlx::query("UPDATE jobs SET lease_expires_at = now() - interval '1 second'")
        .execute(store.pool())
        .await
        .unwrap();
    let second = store.claim_jobs(JobKind::Embed, 1).await.unwrap().remove(0);
    assert_eq!(second.job_id, first.job_id);
    assert_eq!(second.attempts, 2);
    assert_ne!(second.claim_id, first.claim_id);
    assert!(
        !store
            .settle_job(
                &first.job_id,
                first.claim_id.as_deref().unwrap(),
                JobEnd::Succeeded
            )
            .await
            .unwrap()
    );

    // Failures back off, then the fifth quarantines.
    let mut claimed = second;
    for attempt in 2..JOB_ATTEMPT_CAP {
        assert!(
            store
                .settle_job(
                    &claimed.job_id,
                    claimed.claim_id.as_deref().unwrap(),
                    JobEnd::Retry("boom".into())
                )
                .await
                .unwrap()
        );
        assert!(
            store
                .claim_jobs(JobKind::Embed, 1)
                .await
                .unwrap()
                .is_empty(),
            "waiting out the backoff (attempt {attempt})"
        );
        make_due(&db).await;
        claimed = store.claim_jobs(JobKind::Embed, 1).await.unwrap().remove(0);
    }
    assert_eq!(claimed.attempts, JOB_ATTEMPT_CAP);
    assert!(
        store
            .settle_job(
                &claimed.job_id,
                claimed.claim_id.as_deref().unwrap(),
                JobEnd::Retry("boom".into())
            )
            .await
            .unwrap()
    );
    let snapshot = store.job_snapshot().await.unwrap();
    assert_eq!(
        (snapshot.pending, snapshot.running, snapshot.quarantined),
        (0, 0, 1)
    );
    assert_eq!(snapshot.quarantine[0].last_error.as_deref(), Some("boom"));

    // Requeue restarts the attempts; a busy key refuses.
    store.requeue_job(&claimed.job_id).await.unwrap();
    assert!(matches!(
        store.requeue_job(&claimed.job_id).await,
        Err(StoreError::NotFound(_))
    ));
    let again = store.claim_jobs(JobKind::Embed, 1).await.unwrap().remove(0);
    assert_eq!(again.attempts, 1);
    store
        .settle_job(
            &again.job_id,
            again.claim_id.as_deref().unwrap(),
            JobEnd::Quarantine("bad vector".into()),
        )
        .await
        .unwrap();
    store.enqueue_job(JobKind::Embed, "k").await.unwrap();
    assert!(
        matches!(
            store.requeue_job(&again.job_id).await,
            Err(StoreError::Conflict(_))
        ),
        "key already pending"
    );
    store.discard_job(&again.job_id).await.unwrap();
    assert_eq!(store.job_snapshot().await.unwrap().quarantined, 0);
    assert!(store.discard_job(&again.job_id).await.is_err());
    db.finish().await;
}

#[tokio::test]
async fn a_poison_job_that_keeps_losing_its_lease_is_quarantined() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    store.enqueue_job(JobKind::Distill, "k").await.unwrap();
    for _ in 0..JOB_ATTEMPT_CAP {
        assert_eq!(
            store.claim_jobs(JobKind::Distill, 1).await.unwrap().len(),
            1
        );
        sqlx::query("UPDATE jobs SET lease_expires_at = now() - interval '1 second'")
            .execute(store.pool())
            .await
            .unwrap();
    }
    assert!(
        store
            .claim_jobs(JobKind::Distill, 1)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.job_snapshot().await.unwrap().quarantined, 1);
    db.finish().await;
}
