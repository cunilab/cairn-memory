//! Step 6b: the link job, global promotion and its guards, quarantine auto-retry.

use cairn_core::capture::WorkContext;
use cairn_core::memory::Scope;
use cairn_store::testing::{TestDb, rule, save_input};
use sqlx::Row;

fn in_project(id: &str, files: &[&str]) -> WorkContext {
    WorkContext {
        project_id: Some(id.to_owned()),
        files: files.iter().map(|f| (*f).to_owned()).collect(),
        ..WorkContext::default()
    }
}

fn with_files(files: &[&str]) -> WorkContext {
    WorkContext {
        files: files.iter().map(|f| (*f).to_owned()).collect(),
        ..WorkContext::default()
    }
}

/// `(source, target, relation, origin)` of every link, sorted.
async fn links(db: &TestDb) -> Vec<(String, String, String, String)> {
    sqlx::query("SELECT source_id, target_id, relation, origin FROM links ORDER BY created_at, source_id, target_id")
        .fetch_all(db.store.pool())
        .await
        .unwrap()
        .iter()
        .map(|r| {
            (
                r.get("source_id"),
                r.get("target_id"),
                r.get("relation"),
                r.get("origin"),
            )
        })
        .collect()
}

fn tilted(weight: f32, axis: usize) -> Vec<f32> {
    let mut v = vec![0.0; 384];
    v[0] = weight;
    v[axis] = (1.0 - weight * weight).sqrt();
    v
}

#[tokio::test]
async fn link_job_relates_shared_file_within_cohort() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let (a, b) = (db.person("a").await, db.person("b").await);
    let (project, _) = store
        .init_project(&a, "github.com/acme/one", None)
        .await
        .unwrap();
    store
        .join_project(&b, &project.project_id, None)
        .await
        .unwrap();
    let first = store
        .save(
            &a,
            rule("Queue claims use leases"),
            &in_project(&project.project_id, &["src/jobs/queue.rs"]),
        )
        .await
        .unwrap()
        .id;
    let second = store
        .save(
            &b,
            rule("Queue retries are capped"),
            &in_project(&project.project_id, &["src/jobs/queue.rs"]),
        )
        .await
        .unwrap()
        .id;
    let unrelated = store
        .save(
            &a,
            rule("Billing rounds to cents"),
            &in_project(&project.project_id, &["src/billing/money.rs"]),
        )
        .await
        .unwrap()
        .id;
    for id in [&first, &second, &unrelated] {
        store.link_memory(id).await.unwrap();
    }
    let shown = links(&db).await;
    assert_eq!(
        shown.len(),
        1,
        "only the shared file links, across members of one project: {shown:?}"
    );
    let mut ends = [shown[0].0.clone(), shown[0].1.clone()];
    ends.sort();
    let mut want = [first.clone(), second.clone()];
    want.sort();
    assert_eq!(
        (ends, shown[0].2.as_str(), shown[0].3.as_str()),
        (want, "relates", "auto")
    );
    // Running the job again changes nothing.
    assert_eq!(store.link_memory(&second).await.unwrap(), 0);
    db.finish().await;
}

#[tokio::test]
async fn link_job_never_crosses_cohort() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let (a, b) = (db.person("a").await, db.person("b").await);
    let work = with_files(&["src/shared.rs"]);
    let one = store
        .save(&a, rule("Alice private rule"), &work)
        .await
        .unwrap()
        .id;
    let two = store
        .save(&b, rule("Bob private rule"), &work)
        .await
        .unwrap()
        .id;
    store.link_memory(&one).await.unwrap();
    store.link_memory(&two).await.unwrap();
    assert!(
        links(&db).await.is_empty(),
        "two people's personal memories are never linked"
    );

    // Nor does a personal memory kept inside a shared project link to another member's.
    let (project, _) = store
        .init_project(&a, "github.com/acme/two", None)
        .await
        .unwrap();
    store
        .join_project(&b, &project.project_id, None)
        .await
        .unwrap();
    let mut mine = rule("Alice personal in project");
    mine.scope = Some(Scope::Personal);
    let mine = store
        .save(
            &a,
            mine,
            &in_project(&project.project_id, &["src/shared.rs"]),
        )
        .await
        .unwrap()
        .id;
    let theirs = store
        .save(
            &b,
            rule("Bob project rule here"),
            &in_project(&project.project_id, &["src/shared.rs"]),
        )
        .await
        .unwrap()
        .id;
    store.link_memory(&theirs).await.unwrap();
    store.link_memory(&mine).await.unwrap();
    // The raw table: one personal memory of Bob's against Alice's personal memory in the same
    // project must not link, whatever the read path would hide.
    let mut bob_personal = rule("Bob personal in project");
    bob_personal.scope = Some(Scope::Personal);
    let bob_personal = store
        .save(
            &b,
            bob_personal,
            &in_project(&project.project_id, &["src/shared.rs"]),
        )
        .await
        .unwrap()
        .id;
    store.link_memory(&bob_personal).await.unwrap();
    store.link_memory(&mine).await.unwrap();
    let raw = links(&db).await;
    assert!(
        !raw.iter()
            .any(|l| [&l.0, &l.1].contains(&&bob_personal) && [&l.0, &l.1].contains(&&mine)),
        "personal memories of two members never link: {raw:?}"
    );
    // Alice (a project member) may be linked from her personal memory to a project rule; the link
    // is hidden from Bob, who cannot read her personal memory, and no link reaches her first rule.
    let seen_by_bob = store.get(&b, &theirs).await.unwrap();
    assert!(
        seen_by_bob
            .links
            .iter()
            .all(|l| ![&l.source, &l.target].contains(&&mine)),
        "{:?}",
        seen_by_bob.links
    );
    let all = links(&db).await;
    assert!(
        !all.iter()
            .any(|l| [&l.0, &l.1].contains(&&theirs) && [&l.0, &l.1].contains(&&one)),
        "{all:?}"
    );
    db.finish().await;
}

#[tokio::test]
async fn link_job_caps_at_8() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let (project, _) = store
        .init_project(&me, "github.com/acme/hub", None)
        .await
        .unwrap();
    let mut ids = Vec::new();
    for n in 0..12 {
        let id = store
            .save(
                &me,
                rule(&format!("Hub rule number {n}")),
                &in_project(&project.project_id, &["src/hub.rs"]),
            )
            .await
            .unwrap()
            .id;
        ids.push(id);
    }
    for id in &ids {
        store.link_memory(id).await.unwrap();
    }
    let all = links(&db).await;
    let mut most = 0;
    for id in &ids {
        let n = all.iter().filter(|l| &l.0 == id || &l.1 == id).count();
        assert!(n <= 8, "{id} carries {n} auto links");
        most = most.max(n);
    }
    assert_eq!(most, 8, "the cap is what stopped the linking");
    db.finish().await;
}

#[tokio::test]
async fn gotcha_fix_links_procedure() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let (project, _) = store
        .init_project(&me, "github.com/acme/ship", None)
        .await
        .unwrap();
    let work = in_project(&project.project_id, &[]);
    let procedure = store
        .save(
            &me,
            save_input(serde_json::json!({
                "kind": "procedure", "title": "Release checklist", "summary": "How a release is cut and verified.",
                "reason": "Releases broke before.", "fields": {"steps": ["Run `deploy_check` first", "Tag the release"]}})),
            &work,
        )
        .await
        .unwrap()
        .id;
    let gotcha = store
        .save(
            &me,
            save_input(serde_json::json!({
                "kind": "gotcha", "title": "Release fails on stale config", "summary": "The release job exits early when the config is stale.",
                "reason": "Seen twice.", "fields": {"symptom": "job exits", "fix": "Run `deploy_check` before tagging"}})),
            &work,
        )
        .await
        .unwrap()
        .id;
    store.link_memory(&gotcha).await.unwrap();
    assert_eq!(
        links(&db).await,
        [(
            procedure.clone(),
            gotcha.clone(),
            "fixes".into(),
            "auto".into()
        )],
        "the procedure fixes the gotcha, not just relates to it"
    );
    // The reverse direction finds the same pair and adds nothing.
    assert_eq!(store.link_memory(&procedure).await.unwrap(), 0);
    db.finish().await;
}

/// Two owners, two projects, one gotcha learned twice (cosine 0.97). Returns the ids.
async fn twin_gotchas(db: &TestDb) -> (String, String, String, String, String) {
    let store = &db.store;
    let (a, b) = (db.person("a").await, db.person("b").await);
    let (pa, _) = store
        .init_project(&a, "github.com/acme/one", None)
        .await
        .unwrap();
    let (pb, _) = store
        .init_project(&b, "github.com/acme/two", None)
        .await
        .unwrap();
    let gotcha = |title: &str| {
        save_input(serde_json::json!({
            "kind": "gotcha", "title": title, "summary": "Webhooks are delivered twice by the provider.",
            "reason": "Seen in production.", "fields": {"symptom": "double charge", "fix": "Dedupe on the event id"},
            "evidence": ["src/webhooks/replay.rs", "https://internal.example/runbooks/webhooks"]}))
    };
    let one = store
        .save(
            &a,
            gotcha("Webhook replays double charge"),
            &in_project(&pa.project_id, &[]),
        )
        .await
        .unwrap()
        .id;
    let two = store
        .save(
            &b,
            gotcha("Provider replays webhook events"),
            &in_project(&pb.project_id, &[]),
        )
        .await
        .unwrap()
        .id;
    store
        .upsert_embedding(&one, "h1", &tilted(1.0, 1))
        .await
        .unwrap();
    store
        .upsert_embedding(&two, "h2", &tilted(0.97, 2))
        .await
        .unwrap();
    (a, b, one, two, pa.project_id)
}

async fn globals(db: &TestDb) -> Vec<String> {
    sqlx::query_scalar("SELECT memory_id FROM memories WHERE scope = 'global' AND origin = 'system' ORDER BY memory_id")
        .fetch_all(db.store.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn promotion_creates_one_global_across_owners() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let (a, _b, one, two, _) = twin_gotchas(&db).await;
    store.consolidate_memory(&one).await.unwrap();
    store.consolidate_memory(&two).await.unwrap();
    let copies = globals(&db).await;
    assert_eq!(copies.len(), 1, "both twins promote to the same copy");
    let global = &copies[0];
    // The copy carries the lesson only: no one's evidence, and no person as its owner.
    let (owner, evidence): (String, i32) =
        sqlx::query_as("SELECT owner_id, cardinality(evidence) FROM memories WHERE memory_id = $1")
            .bind(global)
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!((owner.as_str(), evidence), ("system", 0));
    let sources: i32 = sqlx::query_scalar(
        "SELECT min(cardinality(evidence)) FROM memories WHERE memory_id = ANY($1)",
    )
    .bind([one.as_str(), two.as_str()])
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert!(sources > 0, "the sources carry evidence the copy must not");
    let mut linked: Vec<_> = links(&db)
        .await
        .into_iter()
        .filter(|l| &l.0 == global)
        .map(|l| (l.1, l.2, l.3))
        .collect();
    linked.sort();
    let mut want = vec![
        (one.clone(), "relates".to_owned(), "auto".to_owned()),
        (two.clone(), "relates".to_owned(), "auto".to_owned()),
    ];
    want.sort();
    assert_eq!(linked, want);
    // A stranger reads the copy but sees neither source in its links.
    let stranger = db.person("stranger").await;
    let seen = store.get(&stranger, global).await.unwrap();
    assert_eq!(
        (seen.scope, seen.title.as_str()),
        (Scope::Global, "Provider replays webhook events")
    );
    assert!(seen.links.is_empty(), "{:?}", seen.links);
    // A promoted gotcha is system-made but has no 180-day clock: it lives while two sources do,
    // so the age sweep cannot expire it for the reconcile pass to revive.
    assert_eq!(seen.expires_at, None);
    assert!(store.get(&stranger, &one).await.is_err());
    // Nobody can forget the copy, and the audit trail has the promotion.
    assert!(store.forget(&stranger, global).await.is_err());
    assert!(store.forget(&a, global).await.is_err());
    let audit = store.list_audit(&a, &Default::default()).await.unwrap();
    assert!(
        audit.iter().any(|e| e.action == "memory_promoted"),
        "{audit:?}"
    );
    db.finish().await;
}

#[tokio::test]
async fn promotion_never_takes_personal_or_other_kinds() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let (a, b) = (db.person("a").await, db.person("b").await);
    let (pa, _) = store
        .init_project(&a, "github.com/acme/one", None)
        .await
        .unwrap();
    let (pb, _) = store
        .init_project(&b, "github.com/acme/two", None)
        .await
        .unwrap();
    let save = |kind: &str, scope: &str, title: &str| {
        save_input(
            serde_json::json!({"kind": kind, "scope": scope, "title": title,
            "summary": "A statement that is the same everywhere.", "reason": "Stated twice on purpose.", "fields": {}}),
        )
    };
    let mut ids = Vec::new();
    // personal rule (even inside a project) vs project rule; project facts; project decisions.
    for (owner, project, kind, scope, title) in [
        (&a, &pa, "rule", "personal", "Personal rule in project"),
        (&b, &pb, "rule", "project", "Project rule elsewhere"),
        (&a, &pa, "fact", "project", "Fact in the first project"),
        (&b, &pb, "fact", "project", "Fact in the second project"),
        (&a, &pa, "decision", "project", "Decision in first project"),
        (&b, &pb, "decision", "project", "Decision in second project"),
    ] {
        let id = store
            .save(
                owner,
                save(kind, scope, title),
                &in_project(&project.project_id, &[]),
            )
            .await
            .unwrap()
            .id;
        store
            .upsert_embedding(&id, title, &tilted(1.0, 1))
            .await
            .unwrap();
        ids.push(id);
    }
    for id in &ids {
        store.consolidate_memory(id).await.unwrap();
    }
    assert!(
        globals(&db).await.is_empty(),
        "no personal text, fact or decision is ever published"
    );
    db.finish().await;
}

#[tokio::test]
async fn promotion_redacts_the_copy() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let (_a, _b, one, two, _) = twin_gotchas(&db).await;
    // A secret that slipped into a stored project memory (for example before a redaction rule existed).
    sqlx::query(
        "UPDATE memories SET summary = 'Key token=sk_live_abcdef123456789 leaked in a log line.',
                title = 'Rotate token=sk_live_abcdef123456789 now',
                reason = 'Seen with password=hunter2hunter2 in prod.',
                evidence = ARRAY['https://x.test/?token=sk_live_abcdef123456789'],
                fields = '{\"symptom\":\"x\",\"fix\":\"rotate password=hunter2hunter2 now\"}' WHERE memory_id = ANY($1)",
    )
    .bind([one.as_str(), two.as_str()])
    .execute(store.pool())
    .await
    .unwrap();
    store.consolidate_memory(&two).await.unwrap();
    let copy = globals(&db).await;
    assert_eq!(copy.len(), 1);
    let row = sqlx::query(
        "SELECT title || ' ' || summary || ' ' || reason || ' ' || fields::text || ' ' || array_to_string(evidence, ' ') AS text
           FROM memories WHERE memory_id = $1",
    )
    .bind(&copy[0])
    .fetch_one(store.pool())
    .await
    .unwrap();
    let text: String = row.get("text");
    for secret in ["sk_live_abcdef123456789", "hunter2hunter2"] {
        assert!(!text.contains(secret), "{text}");
    }
    db.finish().await;
}

#[tokio::test]
async fn promotion_withdrawn_below_two_live_sources() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let (a, b, one, two, _) = twin_gotchas(&db).await;
    store.consolidate_memory(&two).await.unwrap();
    let global = globals(&db).await.remove(0);
    assert!(store.get(&b, &global).await.is_ok());
    // One source is forgotten: fewer than 2 live sources, the copy disappears for everyone.
    store.forget(&a, &one).await.unwrap();
    assert_eq!(store.reconcile_promoted().await.unwrap(), (1, 0));
    for person in [&a, &b] {
        assert!(
            store.get(person, &global).await.is_err(),
            "withdrawn copy is hidden"
        );
    }
    let audit = store.list_audit(&b, &Default::default()).await.unwrap();
    assert!(
        audit.iter().any(|e| e.action == "memory_withdrawn"),
        "{audit:?}"
    );
    // The sweep runs the same check and does not withdraw twice.
    store.sweep().await.unwrap();
    assert_eq!(store.reconcile_promoted().await.unwrap(), (0, 0));
    db.finish().await;
}

#[tokio::test]
async fn promotion_revives_with_two_live_sources() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let (a, b, one, two, _) = twin_gotchas(&db).await;
    store.consolidate_memory(&two).await.unwrap();
    let global = globals(&db).await.remove(0);
    // A source expires: withdrawn. It comes back: the copy is live again.
    sqlx::query(
        "UPDATE memories SET expires_at = now() - interval '1 second' WHERE memory_id = $1",
    )
    .bind(&one)
    .execute(store.pool())
    .await
    .unwrap();
    assert_eq!(store.reconcile_promoted().await.unwrap(), (1, 0));
    assert!(store.get(&b, &global).await.is_err());
    sqlx::query("UPDATE memories SET expires_at = NULL WHERE memory_id = $1")
        .bind(&one)
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(store.reconcile_promoted().await.unwrap(), (0, 1));
    assert!(store.get(&a, &global).await.is_ok());
    // A stale source still counts as live.
    sqlx::query("UPDATE memories SET status = 'stale' WHERE memory_id = $1")
        .bind(&two)
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(store.reconcile_promoted().await.unwrap(), (0, 0));
    db.finish().await;
}

#[tokio::test]
async fn quarantined_jobs_retry_without_a_human() {
    use cairn_core::process::jobs::{JOB_ATTEMPT_CAP, JobEnd, JobKind};
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    store.enqueue_job(JobKind::Link, "mem_x").await.unwrap();
    let job = store.claim_jobs(JobKind::Link, 1).await.unwrap().remove(0);
    store
        .settle_job(
            &job.job_id,
            job.claim_id.as_deref().unwrap(),
            JobEnd::Quarantine("bad".into()),
        )
        .await
        .unwrap();
    store.sweep().await.unwrap();
    assert_eq!(
        store.job_snapshot().await.unwrap().quarantined,
        1,
        "too soon to retry"
    );
    // The cool-down is a day, not an hour.
    sqlx::query(
        "UPDATE jobs SET finished_at = now() - interval '23 hours' WHERE status = 'quarantined'",
    )
    .execute(store.pool())
    .await
    .unwrap();
    store.sweep().await.unwrap();
    assert_eq!(store.job_snapshot().await.unwrap().quarantined, 1);
    sqlx::query(
        "UPDATE jobs SET finished_at = now() - interval '25 hours' WHERE status = 'quarantined'",
    )
    .execute(store.pool())
    .await
    .unwrap();
    store.sweep().await.unwrap();
    let snapshot = store.job_snapshot().await.unwrap();
    assert_eq!((snapshot.pending, snapshot.quarantined), (1, 0));
    // One try is left: a failure quarantines it again.
    let job = store.claim_jobs(JobKind::Link, 1).await.unwrap().remove(0);
    assert_eq!(job.attempts, JOB_ATTEMPT_CAP);
    store
        .settle_job(
            &job.job_id,
            job.claim_id.as_deref().unwrap(),
            JobEnd::Retry("still bad".into()),
        )
        .await
        .unwrap();
    assert_eq!(store.job_snapshot().await.unwrap().quarantined, 1);
    db.finish().await;
}

#[tokio::test]
async fn consolidation_records_memory_merged_for_the_owner() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let (a, b) = (db.person("a").await, db.person("b").await);
    let older = store
        .save(&a, rule("Money is integer cents"), &WorkContext::default())
        .await
        .unwrap()
        .id;
    let newer = store
        .save(
            &a,
            rule("Money is stored as cents"),
            &WorkContext::default(),
        )
        .await
        .unwrap()
        .id;
    for (id, hash, vector) in [
        (&older, "h1", tilted(1.0, 1)),
        (&newer, "h2", tilted(0.97, 2)),
    ] {
        store.upsert_embedding(id, hash, &vector).await.unwrap();
    }
    store.consolidate_memory(&older).await.unwrap();
    store.consolidate_memory(&newer).await.unwrap();
    let events = store.list_audit(&a, &Default::default()).await.unwrap();
    let merged: Vec<_> = events
        .iter()
        .filter(|e| e.action == "memory_merged")
        .collect();
    assert_eq!(merged.len(), 1, "{events:?}");
    assert_eq!(merged[0].subject_id.as_deref(), Some(newer.as_str()));
    assert_eq!(merged[0].metadata["into"], newer.as_str());
    assert_eq!(merged[0].metadata["merged"], serde_json::json!([older]));
    assert_eq!(merged[0].titles[&newer], "Money is stored as cents");
    assert!(
        store
            .list_audit(&b, &Default::default())
            .await
            .unwrap()
            .is_empty()
    );
    db.finish().await;
}

#[tokio::test]
async fn quarantined_jobs_retry_once_after_an_upgrade() {
    use cairn_core::process::jobs::{JobEnd, JobKind};
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    store.enqueue_job(JobKind::Link, "mem_x").await.unwrap();
    let job = store.claim_jobs(JobKind::Link, 1).await.unwrap().remove(0);
    store
        .settle_job(
            &job.job_id,
            job.claim_id.as_deref().unwrap(),
            JobEnd::Quarantine("bad".into()),
        )
        .await
        .unwrap();
    // The first start only records the version; a restart on the same version retries nothing.
    for _ in 0..2 {
        assert_eq!(
            store
                .retry_quarantined_after_upgrade("0.1.5")
                .await
                .unwrap(),
            0
        );
    }
    assert_eq!(store.job_snapshot().await.unwrap().quarantined, 1);
    // A new version retries it at once, whatever its age, and only once.
    assert_eq!(
        store
            .retry_quarantined_after_upgrade("0.1.6")
            .await
            .unwrap(),
        1
    );
    let snapshot = store.job_snapshot().await.unwrap();
    assert_eq!((snapshot.pending, snapshot.quarantined), (1, 0));
    let job = store.claim_jobs(JobKind::Link, 1).await.unwrap().remove(0);
    store
        .settle_job(
            &job.job_id,
            job.claim_id.as_deref().unwrap(),
            JobEnd::Quarantine("still bad".into()),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .retry_quarantined_after_upgrade("0.1.6")
            .await
            .unwrap(),
        0
    );
    assert_eq!(store.job_snapshot().await.unwrap().quarantined, 1);
    db.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_promotion_makes_one_copy() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (_a, _b, one, two, _) = twin_gotchas(&db).await;
    let store = &db.store;
    for _ in 0..3 {
        let (x, y) = tokio::join!(store.promote_memory(&one), store.promote_memory(&two));
        x.unwrap();
        y.unwrap();
    }
    assert_eq!(globals(&db).await.len(), 1, "racing twins share one copy");
    db.finish().await;
}

#[tokio::test]
async fn a_copy_not_yet_linked_is_joined_by_cosine() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let (_a, _b, one, two, _) = twin_gotchas(&db).await;
    store.consolidate_memory(&two).await.unwrap();
    let global = globals(&db).await.remove(0);
    // The copy is embedded but its links are gone: only the vector can find it.
    sqlx::query("DELETE FROM links WHERE source_id = $1")
        .bind(&global)
        .execute(store.pool())
        .await
        .unwrap();
    store
        .upsert_embedding(&global, "hg", &tilted(1.0, 1))
        .await
        .unwrap();
    let c = db.person("c").await;
    let (pc, _) = store
        .init_project(&c, "github.com/acme/three", None)
        .await
        .unwrap();
    let three = store
        .save(
            &c,
            save_input(serde_json::json!({
                "kind": "gotcha", "title": "Webhook retries charge twice",
                "summary": "The provider redelivers webhooks.", "reason": "Seen in production.",
                "fields": {"symptom": "double charge", "fix": "Dedupe on the event id"}})),
            &in_project(&pc.project_id, &[]),
        )
        .await
        .unwrap()
        .id;
    store
        .upsert_embedding(&three, "h3", &tilted(0.98, 3))
        .await
        .unwrap();
    assert_eq!(
        store.promote_memory(&three).await.unwrap().as_deref(),
        Some(global.as_str())
    );
    assert_eq!(
        globals(&db).await,
        std::slice::from_ref(&global),
        "no second copy"
    );
    let linked: Vec<String> = links(&db)
        .await
        .into_iter()
        .filter(|l| l.0 == global)
        .map(|l| l.1)
        .collect();
    assert!(linked.contains(&three), "{linked:?}");
    assert!(linked.contains(&one) || linked.contains(&two), "{linked:?}");
    db.finish().await;
}

/// A twin hides behind more same-project neighbours than the index scan returns by default.
#[tokio::test]
async fn twin_search_finds_a_twin_behind_a_crowd_of_nearer_neighbours() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    // A second pool that forbids scans and sorts the HNSW index cannot replace, so the planner must
    // use it (a tiny test table would otherwise be read in full).
    let url = format!(
        "{}{}options=-c%20enable_seqscan%3Doff%20-c%20enable_bitmapscan%3Doff%20-c%20enable_sort%3Doff",
        db.url(),
        if db.url().contains('?') { "&" } else { "?" }
    );
    let indexed = cairn_store::Store::connect(&url, db.store.schema())
        .await
        .unwrap();
    let (a, b) = (db.person("a").await, db.person("b").await);
    let (pa, _) = indexed
        .init_project(&a, "github.com/acme/one", None)
        .await
        .unwrap();
    let (pb, _) = indexed
        .init_project(&b, "github.com/acme/two", None)
        .await
        .unwrap();
    let gotcha = |title: String| {
        save_input(serde_json::json!({
            "kind": "gotcha", "title": title, "summary": "Webhooks are delivered twice.",
            "reason": "Seen in production.", "fields": {"symptom": "double charge", "fix": "Dedupe on the event id"}}))
    };
    let subject = indexed
        .save(
            &a,
            gotcha("Webhook replays charge twice".into()),
            &in_project(&pa.project_id, &[]),
        )
        .await
        .unwrap()
        .id;
    indexed
        .upsert_embedding(&subject, "s", &tilted(1.0, 1))
        .await
        .unwrap();
    // 110 neighbours in the subject's own project, all nearer than the twin (which sits beside one
    // of them, so the graph can reach it).
    for n in 0..110 {
        let id = indexed
            .save(
                &a,
                gotcha(format!("Crowd gotcha number {n}")),
                &in_project(&pa.project_id, &[]),
            )
            .await
            .unwrap()
            .id;
        indexed
            .upsert_embedding(&id, &format!("c{n}"), &tilted(0.97, 2 + n))
            .await
            .unwrap();
    }
    let twin = indexed
        .save(
            &b,
            gotcha("Provider replays webhook events".into()),
            &in_project(&pb.project_id, &[]),
        )
        .await
        .unwrap()
        .id;
    indexed
        .upsert_embedding(&twin, "t", &tilted(0.95, 2))
        .await
        .unwrap();
    sqlx::raw_sql("ANALYZE memories")
        .execute(indexed.pool())
        .await
        .unwrap();
    assert!(
        indexed.promote_memory(&subject).await.unwrap().is_some(),
        "the twin is found past the first ef_search (100) index hits"
    );
    indexed.pool().close().await;
    db.finish().await;
}

#[tokio::test]
async fn the_link_cohort_excludes_expired_memories_and_promoted_copies() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let (_a, _b, _one, two, pa) = twin_gotchas(&db).await;
    store.consolidate_memory(&two).await.unwrap();
    let global = globals(&db).await.remove(0);
    let a = db.person("c").await;
    store.join_project(&a, &pa, None).await.unwrap();
    let work = in_project(&pa, &["src/shared.rs"]);
    let live = store
        .save(&a, rule("Shared file rule one"), &work)
        .await
        .unwrap()
        .id;
    let gone = store
        .save(&a, rule("Shared file rule two"), &work)
        .await
        .unwrap()
        .id;
    let fresh = store
        .save(&a, rule("Shared file rule three"), &work)
        .await
        .unwrap()
        .id;
    sqlx::query(
        "UPDATE memories SET expires_at = now() - interval '1 second' WHERE memory_id = $1",
    )
    .bind(&gone)
    .execute(store.pool())
    .await
    .unwrap();
    // The promoted copy cites the same file, yet no rule links to it.
    sqlx::query(
        "INSERT INTO entities (memory_id, kind, name) VALUES ($1, 'file', 'src/shared.rs')",
    )
    .bind(&global)
    .execute(store.pool())
    .await
    .unwrap();
    store.link_memory(&fresh).await.unwrap();
    store.link_memory(&live).await.unwrap();
    let shown: Vec<_> = links(&db)
        .await
        .into_iter()
        .filter(|l| [&l.0, &l.1].iter().any(|id| [&live, &fresh].contains(id)))
        .collect();
    assert_eq!(shown.len(), 1, "only live with fresh: {shown:?}");
    assert!(
        shown
            .iter()
            .all(|l| ![&l.0, &l.1].contains(&&gone) && ![&l.0, &l.1].contains(&&global))
    );
    db.finish().await;
}

#[tokio::test]
async fn a_stale_copy_that_lapses_is_not_revived_by_reconcile() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let (_a, _b, _one, two, _) = twin_gotchas(&db).await;
    store.consolidate_memory(&two).await.unwrap();
    let global = globals(&db).await.remove(0);
    sqlx::query(
        "UPDATE memories SET status = 'stale', stale_cause = 'feedback',
                status_changed_at = now() - interval '100 days' WHERE memory_id = $1",
    )
    .bind(&global)
    .execute(store.pool())
    .await
    .unwrap();
    store.sweep().await.unwrap();
    let after = |store: &cairn_store::Store| {
        let (store, global) = (store.clone(), global.clone());
        async move {
            sqlx::query_as::<_, (bool, String)>(
                "SELECT coalesce(expires_at <= now(), false), updated_at::text FROM memories WHERE memory_id = $1",
            )
            .bind(global)
            .fetch_one(store.pool())
            .await
            .unwrap()
        }
    };
    let first = after(store).await;
    assert!(
        first.0,
        "the lapsed copy stays withdrawn: its two sources are live but it was flagged wrong"
    );
    store.sweep().await.unwrap();
    assert_eq!(after(store).await, first, "a second sweep changes nothing");
    db.finish().await;
}

#[tokio::test]
async fn quarantined_twins_of_one_key_do_not_fail_the_sweep() {
    use cairn_core::process::jobs::{JobEnd, JobKind};
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    for round in 0..2 {
        store.enqueue_job(JobKind::Link, "mem_x").await.unwrap();
        let job = store.claim_jobs(JobKind::Link, 1).await.unwrap().remove(0);
        store
            .settle_job(
                &job.job_id,
                job.claim_id.as_deref().unwrap(),
                JobEnd::Quarantine(format!("bad {round}")),
            )
            .await
            .unwrap();
    }
    assert_eq!(store.job_snapshot().await.unwrap().quarantined, 2);
    sqlx::query(
        "UPDATE jobs SET finished_at = now() - interval '25 hours' WHERE status = 'quarantined'",
    )
    .execute(store.pool())
    .await
    .unwrap();
    store
        .sweep()
        .await
        .expect("the retry cannot trip jobs_one_pending");
    let snapshot = store.job_snapshot().await.unwrap();
    assert_eq!((snapshot.pending, snapshot.quarantined), (1, 0));
    db.finish().await;
}
