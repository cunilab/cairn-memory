//! Step 6b: the link job, global promotion and its guards, quarantine auto-retry.

use cairn_core::capture::WorkContext;
use cairn_core::memory::{Kind, Scope};
use cairn_store::memory::ListFilter;
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
    assert_eq!(store.reconcile_promoted().await.unwrap(), (0, 0));
    for person in [&a, &b] {
        let copy = store.get(person, &global).await.expect("copy stays active");
        assert_eq!(copy.status, cairn_core::memory::Status::Active);
    }
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
    assert_eq!(store.reconcile_promoted().await.unwrap(), (0, 0));
    assert!(
        store.get(&b, &global).await.is_ok(),
        "an old source is still live"
    );
    sqlx::query("UPDATE memories SET expires_at = NULL WHERE memory_id = $1")
        .bind(&one)
        .execute(store.pool())
        .await
        .unwrap();
    assert_eq!(store.reconcile_promoted().await.unwrap(), (0, 0));
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
    assert!(
        shown.iter().any(|l| [&l.0, &l.1].contains(&&gone)),
        "age does not drop a memory from the link cohort: {shown:?}"
    );
    assert!(
        shown.iter().all(|l| ![&l.0, &l.1].contains(&&global)),
        "a promoted copy stays out: {shown:?}"
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
        !first.0,
        "sweep does not expire a stale promoted copy: {first:?}"
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
    // Quarantining keeps one row per key, so the twin is written the way an older server left it.
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
    sqlx::query(
        "INSERT INTO jobs (job_id, kind, job_key, status, attempts, finished_at)
         VALUES ('job_twin', 'link', 'mem_x', 'quarantined', 5, now())",
    )
    .execute(store.pool())
    .await
    .unwrap();
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

async fn column_rank(store: &cairn_store::Store, id: &str) -> String {
    sqlx::query_scalar("SELECT rank FROM memories WHERE memory_id = $1")
        .bind(id)
        .fetch_one(store.pool())
        .await
        .unwrap()
}

async fn column_proof(store: &cairn_store::Store, id: &str) -> i32 {
    sqlx::query_scalar("SELECT proof_count FROM memories WHERE memory_id = $1")
        .bind(id)
        .fetch_one(store.pool())
        .await
        .unwrap()
}

fn capture(
    event: &str,
    session: &str,
    kind: &str,
    content: &str,
    ok: Option<bool>,
    project: Option<&str>,
    file: Option<&str>,
) -> cairn_core::capture::ObservationInput {
    let mut context = serde_json::json!({
        "repository": "github.com/acme/one",
        "branch": "feat",
        "worktree": "wt-1",
    });
    if kind == "command" {
        context["commands"] = serde_json::json!([content.lines().next().unwrap_or("")]);
    }
    if let Some(file) = file {
        context["files"] = serde_json::json!([file]);
    }
    if let Some(project) = project {
        context["project_id"] = serde_json::json!(project);
    } else {
        // No project id, and not the person's repository: the fallback must not attach it.
        context["repository"] = serde_json::json!("local:scratch");
    }
    serde_json::from_value(serde_json::json!({
        "event_id": event,
        "session_id": session,
        "kind": kind,
        "ok": ok,
        "content": content,
        "context": context,
    }))
    .unwrap()
}

async fn distill_fail_fix(
    store: &cairn_store::Store,
    person: &str,
    session: &str,
    project: Option<&str>,
) {
    let fail = "cargo test\nexit 101: error[E0308]: mismatched types in src/billing/money.rs";
    store
        .capture(
            person,
            capture(
                &format!("{session}-c1"),
                session,
                "command",
                fail,
                Some(false),
                project,
                None,
            ),
        )
        .await
        .unwrap();
    store
        .capture(
            person,
            capture(
                &format!("{session}-e1"),
                session,
                "edit",
                "Edit src/billing/money.rs\nx",
                None,
                project,
                Some("src/billing/money.rs"),
            ),
        )
        .await
        .unwrap();
    store
        .capture(
            person,
            capture(
                &format!("{session}-c2"),
                session,
                "command",
                "cargo test",
                Some(true),
                project,
                None,
            ),
        )
        .await
        .unwrap();
    store
        .capture(
            person,
            capture(
                &format!("{session}-a1"),
                session,
                "assistant",
                "Decision: keep the queue in Postgres\nReason: one database is enough here",
                None,
                project,
                None,
            ),
        )
        .await
        .unwrap();
    store.end_session(person, session).await.unwrap();
    store.distill_session(person, session).await.unwrap();
}

#[tokio::test]
async fn distilled_notes_take_project_scope_and_candidate_rank() {
    use cairn_core::memory::Kind;
    use cairn_core::process::Origin;
    use cairn_store::memory::ListFilter;

    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let person = db.person("a").await;
    let (project, _) = store
        .init_project(&person, "github.com/acme/one", None)
        .await
        .unwrap();
    let pid = project.project_id.clone();
    distill_fail_fix(store, &person, "in-project", Some(&pid)).await;
    distill_fail_fix(store, &person, "no-project", None).await;

    let all = store
        .list(&person, &ListFilter::default())
        .await
        .unwrap()
        .memories;
    let gotcha = all
        .iter()
        .find(|m| m.kind == Kind::Gotcha && m.project_id.as_deref() == Some(pid.as_str()))
        .expect("project gotcha");
    assert_eq!(gotcha.scope, Scope::Project, "project session");
    assert_eq!(column_rank(store, &gotcha.id).await, "candidate");
    assert_eq!(gotcha.origin, Origin::System);
    for kind in [Kind::Fact, Kind::Decision] {
        let note = all
            .iter()
            .find(|m| m.kind == kind && m.project_id.as_deref() == Some(pid.as_str()))
            .unwrap_or_else(|| panic!("project {kind:?}"));
        assert_eq!(note.scope, Scope::Project);
        assert_eq!(column_rank(store, &note.id).await, "candidate");
        assert_eq!(note.origin, Origin::System);
    }
    let lone = all
        .iter()
        .find(|m| m.kind == Kind::Gotcha && m.project_id.is_none())
        .expect("personal gotcha");
    assert_eq!(lone.scope, Scope::Personal);
    assert_eq!(column_rank(store, &lone.id).await, "candidate");
    let handoffs: Vec<_> = all.iter().filter(|m| m.kind == Kind::Handoff).collect();
    assert_eq!(handoffs.len(), 2);
    for handoff in handoffs {
        assert_eq!(handoff.scope, Scope::Personal);
        assert_eq!(column_rank(store, &handoff.id).await, "normal");
    }
    let counts = store
        .project_counts(std::slice::from_ref(&pid))
        .await
        .unwrap();
    let project_notes = all
        .iter()
        .filter(|m| m.scope == Scope::Project && m.project_id.as_deref() == Some(pid.as_str()))
        .count() as i64;
    assert!(project_notes >= 1);
    assert_eq!(counts[&pid].memories, project_notes);
    assert_eq!(counts[&pid].candidates, project_notes);
    db.finish().await;
}

#[tokio::test]
async fn using_a_candidate_promotes_it() {
    use cairn_core::memory::Kind;
    use cairn_store::memory::{ListFilter, Vote};

    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let person = db.person("a").await;
    let (project, _) = store
        .init_project(&person, "github.com/acme/one", None)
        .await
        .unwrap();
    let pid = project.project_id;
    distill_fail_fix(store, &person, "s1", Some(&pid)).await;
    let all = store
        .list(&person, &ListFilter::default())
        .await
        .unwrap()
        .memories;
    let id = |kind| {
        all.iter()
            .find(|m| m.kind == kind)
            .unwrap_or_else(|| panic!("{kind:?}"))
            .id
            .clone()
    };
    let (gotcha, fact, decision) = (id(Kind::Gotcha), id(Kind::Fact), id(Kind::Decision));
    assert_eq!(column_rank(store, &gotcha).await, "candidate");
    store
        .mark_fetched(std::slice::from_ref(&gotcha))
        .await
        .unwrap();
    assert_eq!(column_rank(store, &gotcha).await, "normal");
    assert_eq!(column_rank(store, &fact).await, "candidate");
    store.feedback(&person, &fact, Vote::Helpful).await.unwrap();
    assert_eq!(column_rank(store, &fact).await, "normal");
    assert_eq!(column_rank(store, &decision).await, "candidate");
    store
        .save(
            &person,
            save_input(serde_json::json!({
                "kind": "decision",
                "title": "keep the queue in Postgres",
                "summary": "decision noted in a session: keep the queue in Postgres",
                "reason": "one database is enough here",
                "scope": "project",
            })),
            &WorkContext {
                project_id: Some(pid.clone()),
                repository: Some("github.com/acme/one".into()),
                branch: Some("feat".into()),
                worktree: Some("wt-1".into()),
                ..WorkContext::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(column_rank(store, &decision).await, "normal");
    let decisions = store
        .list(
            &person,
            &ListFilter {
                kind: Some(Kind::Decision),
                ..ListFilter::default()
            },
        )
        .await
        .unwrap()
        .memories;
    assert_eq!(decisions.len(), 1);
    db.finish().await;
}

#[tokio::test]
async fn restatement_promotes_a_candidate() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let person = db.person("a").await;
    let (project, _) = store
        .init_project(&person, "github.com/acme/one", None)
        .await
        .unwrap();
    let work = in_project(&project.project_id, &[]);
    let older = store
        .save(&person, rule("Queue claims use leases"), &work)
        .await
        .unwrap()
        .id;
    let newer = store
        .save(&person, rule("Queue claims still use leases"), &work)
        .await
        .unwrap()
        .id;
    store
        .upsert_embedding(&older, "h1", &tilted(1.0, 1))
        .await
        .unwrap();
    store
        .upsert_embedding(&newer, "h2", &tilted(0.99, 1))
        .await
        .unwrap();
    sqlx::query("UPDATE memories SET rank = 'candidate' WHERE memory_id = $1")
        .bind(&newer)
        .execute(store.pool())
        .await
        .unwrap();
    let out = store.consolidate_memory(&older).await.unwrap();
    assert_eq!(out.superseded, vec![older.clone()]);
    assert_eq!(column_rank(store, &newer).await, "normal");
    assert_eq!(column_rank(store, &older).await, "normal");
    assert_eq!(column_proof(store, &newer).await, 2);
    assert_eq!(column_proof(store, &older).await, 1);
    db.finish().await;
}

#[tokio::test]
async fn duplicate_saves_count_proofs() {
    use cairn_core::memory::model::Rank;
    use cairn_core::retrieve::pack::{Hit, pack};

    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let person = db.person("a").await;
    let input = save_input(serde_json::json!({
        "kind": "gotcha",
        "title": "Retry the webhook once",
        "summary": "The provider delivers the same webhook twice after a timeout.",
        "reason": "A second delivery double-charges the customer.",
        "fields": {"symptom": "two charges", "cause": "a retry", "fix": "dedupe the event id"}
    }));
    let work = WorkContext::default();
    let first = store.save(&person, input.clone(), &work).await.unwrap();
    assert!(!first.deduplicated);
    sqlx::query("UPDATE memories SET rank = 'candidate' WHERE memory_id = $1")
        .bind(&first.id)
        .execute(store.pool())
        .await
        .unwrap();
    let second = store.save(&person, input.clone(), &work).await.unwrap();
    let third = store.save(&person, input, &work).await.unwrap();
    assert!(second.deduplicated && third.deduplicated);
    assert_eq!(second.id, first.id);
    assert_eq!(third.id, first.id);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM memories WHERE memory_id = $1")
        .bind(&first.id)
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(rows, 1);
    assert_eq!(column_proof(store, &first.id).await, 3);
    assert_eq!(column_rank(store, &first.id).await, "normal");
    let memory = store.get(&person, &first.id).await.unwrap();
    assert_eq!(memory.rank, Rank::Normal);
    let cards = pack(&[Hit { memory, score: 1.0 }], 5, 1200);
    let json = serde_json::to_value(&cards).unwrap();
    assert_eq!(json["results"][0]["seen"], 3);
    db.finish().await;
}

/// Cosine is high enough to supersede, but `node 20` and `node 22` are different rare tokens.
#[tokio::test]
async fn rare_tokens_relate_instead_of_superseding() {
    use cairn_core::memory::Status;
    use cairn_core::memory::model::Rank;
    use cairn_core::retrieve::filter::Filter;

    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let person = db.person("a").await;
    let work = WorkContext::default();
    let older = store
        .save(&person, rule("Use node 20 for the desktop app"), &work)
        .await
        .unwrap()
        .id;
    let newer = store
        .save(&person, rule("Use node 22 for the desktop app"), &work)
        .await
        .unwrap()
        .id;
    store
        .upsert_embedding(&older, "h1", &tilted(1.0, 1))
        .await
        .unwrap();
    store
        .upsert_embedding(&newer, "h2", &tilted(0.97, 2))
        .await
        .unwrap();
    let out = store.consolidate_memory(&newer).await.unwrap();
    assert!(out.superseded.is_empty(), "{out:?}");
    assert_eq!(out.related, vec![older.clone()]);
    let linked = links(&db).await;
    assert!(
        linked.iter().any(|link| {
            link.2 == "relates"
                && link.3 == "auto"
                && ((link.0 == newer && link.1 == older) || (link.0 == older && link.1 == newer))
        }),
        "{linked:?}"
    );
    for id in [&older, &newer] {
        let memory = store.get(&person, id).await.unwrap();
        assert_eq!(memory.status, Status::Active);
        assert_eq!(memory.rank, Rank::Normal);
    }
    let cards = store
        .search(
            &person,
            "desktop app",
            Some(&tilted(1.0, 1)),
            5,
            1200,
            &Filter::default(),
        )
        .await
        .unwrap();
    let ids: Vec<&str> = cards.results.iter().map(|card| card.id.as_str()).collect();
    assert!(
        ids.contains(&older.as_str()) && ids.contains(&newer.as_str()),
        "{ids:?}"
    );
    db.finish().await;
}

async fn handoff_stamp(store: &cairn_store::Store, id: &str) -> (String, String) {
    let row =
        sqlx::query("SELECT updated_at::text AS at, summary FROM memories WHERE memory_id = $1")
            .bind(id)
            .fetch_one(store.pool())
            .await
            .unwrap();
    (row.get("at"), row.get("summary"))
}

async fn session_origin(store: &cairn_store::Store, person: &str, session: &str) -> String {
    let context: serde_json::Value =
        sqlx::query_scalar("SELECT context FROM sessions WHERE person_id = $1 AND session_id = $2")
            .bind(person)
            .bind(session)
            .fetch_one(store.pool())
            .await
            .unwrap();
    context["origin"].as_str().unwrap_or("").to_owned()
}

/// One checkout's captures, with a file name that makes this session's handoff its own.
async fn distill_checkout(
    store: &cairn_store::Store,
    person: &str,
    session: &str,
    file: &str,
    project: &str,
    origin: Option<&str>,
) -> (bool, usize) {
    let take =
        |event: String, kind: &str, content: String, ok: Option<bool>, file: Option<&str>| {
            let mut input = capture(&event, session, kind, &content, ok, Some(project), file);
            input.context.origin = origin.map(str::to_owned);
            input
        };
    store
        .capture(
            person,
            take(
                format!("{session}-c1"),
                "command",
                format!("cargo test\nexit 101: error[E0308]: mismatched types in {file}"),
                Some(false),
                None,
            ),
        )
        .await
        .unwrap();
    store
        .capture(
            person,
            take(
                format!("{session}-e1"),
                "edit",
                format!("Edit {file}\nx"),
                None,
                Some(file),
            ),
        )
        .await
        .unwrap();
    store
        .capture(
            person,
            take(
                format!("{session}-c2"),
                "command",
                "cargo test".to_owned(),
                Some(true),
                None,
            ),
        )
        .await
        .unwrap();
    store
        .capture(
            person,
            take(
                format!("{session}-a1"),
                "assistant",
                format!("Decision: keep {file} in Postgres\nReason: one database is enough here"),
                None,
                None,
            ),
        )
        .await
        .unwrap();
    store.end_session(person, session).await.unwrap();
    let report = store.distill_session(person, session).await.unwrap();
    (report.handoff, report.notes)
}

#[tokio::test]
async fn automated_session_never_overwrites_handoff() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let person = db.person("a").await;
    let (project, _) = store
        .init_project(&person, "github.com/acme/one", None)
        .await
        .unwrap();
    let pid = project.project_id;
    distill_checkout(store, &person, "person", "src/billing/money.rs", &pid, None).await;
    let id: String = sqlx::query_scalar(
        "SELECT memory_id FROM memories WHERE owner_id = $1 AND kind = 'handoff'",
    )
    .bind(&person)
    .fetch_one(store.pool())
    .await
    .unwrap();
    let before = handoff_stamp(store, &id).await;

    let (handoff, saved) = distill_checkout(
        store,
        &person,
        "auto",
        "src/billing/tax.rs",
        &pid,
        Some("automation"),
    )
    .await;
    let after = handoff_stamp(store, &id).await;
    let origin = session_origin(store, &person, "auto").await;
    assert!(
        !handoff && saved > 0 && after == before && origin == "automation",
        "handoff_written={handoff} notes={saved} origin={origin} same_row={}",
        after == before
    );
    let candidates: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memories WHERE kind <> 'handoff' AND rank = 'candidate'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert!(
        candidates >= i64::try_from(saved).unwrap(),
        "notes stay candidates"
    );
    let events = store
        .list_audit(&person, &Default::default())
        .await
        .unwrap();
    let automated = events
        .iter()
        .filter(|event| {
            event.action == "session_distilled"
                && event.metadata["automated"] == serde_json::json!(true)
        })
        .count();
    assert_eq!(automated, 1, "{events:?}");

    let (later_handoff, _) =
        distill_checkout(store, &person, "later", "src/billing/later.rs", &pid, None).await;
    assert!(
        later_handoff,
        "a later normal session still writes the handoff"
    );
    let handoffs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memories WHERE owner_id = $1 AND kind = 'handoff'",
    )
    .bind(&person)
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(handoffs, 2);
    assert_eq!(handoff_stamp(store, &id).await.1, before.1);
    db.finish().await;
}

#[derive(Debug, PartialEq)]
struct HandoffRow {
    at: String,
    summary: String,
    fields: serde_json::Value,
}

async fn handoff_row(store: &cairn_store::Store, id: &str) -> HandoffRow {
    let row = sqlx::query(
        "SELECT updated_at::text AS at, summary, fields FROM memories WHERE memory_id = $1",
    )
    .bind(id)
    .fetch_one(store.pool())
    .await
    .unwrap();
    HandoffRow {
        at: row.get("at"),
        summary: row.get("summary"),
        fields: row.get("fields"),
    }
}

/// A rich handoff stays when a later chat in the same checkout has no files and no open
/// items. A later session that edits a file still replaces it.
#[tokio::test]
async fn chat_session_keeps_richer_handoff() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let person = db.person("a").await;
    let (project, _) = store
        .init_project(&person, "github.com/acme/one", None)
        .await
        .unwrap();
    let pid = project.project_id;
    for (event, file) in [("a-e1", "src/a.rs"), ("a-e2", "src/b.rs")] {
        store
            .capture(
                &person,
                capture(
                    event,
                    "rich",
                    "edit",
                    &format!("Edit {file}\nx"),
                    None,
                    Some(&pid),
                    Some(file),
                ),
            )
            .await
            .unwrap();
    }
    store
        .capture(
            &person,
            capture(
                "a-p1",
                "rich",
                "prompt",
                "TODO: fix flaky test",
                None,
                Some(&pid),
                None,
            ),
        )
        .await
        .unwrap();
    store.end_session(&person, "rich").await.unwrap();
    let written = store.distill_session(&person, "rich").await.unwrap();
    assert!(written.handoff, "the rich session writes the handoff");
    let id: String = sqlx::query_scalar(
        "SELECT memory_id FROM memories WHERE owner_id = $1 AND kind = 'handoff'",
    )
    .bind(&person)
    .fetch_one(store.pool())
    .await
    .unwrap();
    let before = handoff_row(store, &id).await;
    assert_eq!(before.fields["files"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        before.fields["open_questions"].as_array().map(Vec::len),
        Some(1)
    );

    store
        .capture(
            &person,
            capture(
                "b-p1",
                "chat",
                "prompt",
                "we should maybe adjust the roadmap, need to think\nnext: revisit later",
                None,
                Some(&pid),
                None,
            ),
        )
        .await
        .unwrap();
    store.end_session(&person, "chat").await.unwrap();
    let chat = store.distill_session(&person, "chat").await.unwrap();
    let after = handoff_row(store, &id).await;
    let kept_audit = store
        .list_audit(&person, &Default::default())
        .await
        .unwrap()
        .into_iter()
        .any(|event| {
            event.action == "session_distilled"
                && event.metadata["handoff_kept"] == serde_json::json!(true)
        });
    assert!(
        chat.handoff_kept
            && chat.handoff
            && after.summary == before.summary
            && after.fields == before.fields
            && kept_audit,
        "kept={} wrote={} audit={}",
        chat.handoff_kept,
        chat.handoff,
        kept_audit
    );

    store
        .capture(
            &person,
            capture(
                "c-e1",
                "edits",
                "edit",
                "Edit src/c.rs\nx",
                None,
                Some(&pid),
                Some("src/c.rs"),
            ),
        )
        .await
        .unwrap();
    store.end_session(&person, "edits").await.unwrap();
    let edited = store.distill_session(&person, "edits").await.unwrap();
    assert!(
        edited.handoff && !edited.handoff_kept,
        "an edit session replaces the handoff"
    );
    let replaced: serde_json::Value = sqlx::query_scalar(
        "SELECT fields FROM memories WHERE owner_id = $1 AND kind = 'handoff' AND status = 'active'",
    )
    .bind(&person)
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert!(
        replaced["files"]
            .as_array()
            .is_some_and(|files| files.iter().any(|file| file == "src/c.rs")),
        "replaced files: {replaced}"
    );
    db.finish().await;
}

#[tokio::test]
async fn digest_is_project_candidate_with_note_ttl() {
    use cairn_core::memory::Kind;
    use cairn_core::process::Origin;
    use cairn_store::memory::ListFilter;

    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let person = db.person("a").await;
    let (project, _) = store
        .init_project(&person, "github.com/acme/one", None)
        .await
        .unwrap();
    let pid = project.project_id;
    let session = "digest";
    let fail = "cargo test\nexit 101: error[E0308]: mismatched types in src/ui/LedControls.tsx";
    store
        .capture(
            &person,
            capture(
                "d-p1",
                session,
                "prompt",
                "zephyr-quilt: do not copy this prompt into the digest",
                None,
                Some(&pid),
                None,
            ),
        )
        .await
        .unwrap();
    store
        .capture(
            &person,
            capture(
                "d-c1",
                session,
                "command",
                fail,
                Some(false),
                Some(&pid),
                None,
            ),
        )
        .await
        .unwrap();
    for (event, file) in [
        ("d-e1", "src/ui/LedControls.tsx"),
        ("d-e2", "src/panel/removed.tsx"),
    ] {
        store
            .capture(
                &person,
                capture(
                    event,
                    session,
                    "edit",
                    &format!("Edit {file}\nx"),
                    None,
                    Some(&pid),
                    Some(file),
                ),
            )
            .await
            .unwrap();
    }
    store
        .capture(
            &person,
            capture(
                "d-c2",
                session,
                "command",
                "cargo test",
                Some(true),
                Some(&pid),
                None,
            ),
        )
        .await
        .unwrap();
    store.end_session(&person, session).await.unwrap();
    store.distill_session(&person, session).await.unwrap();
    let notes: Vec<_> = store
        .list(&person, &ListFilter::default())
        .await
        .unwrap()
        .memories
        .into_iter()
        .filter(|memory| memory.kind == Kind::Note)
        .collect();
    assert_eq!(notes.len(), 1, "{notes:?}");
    let note = &notes[0];
    assert_eq!(note.scope, Scope::Project);
    assert_eq!(note.origin, Origin::System);
    assert_eq!(column_rank(store, &note.id).await, "candidate");
    assert!(note.expires_at.is_none(), "a digest does not expire");
    assert!(
        note.title.starts_with("Session ") && note.title.ends_with(" on feat"),
        "{}",
        note.title
    );
    assert_eq!(
        note.summary,
        "Changed src/ui/LedControls.tsx, src/panel/removed.tsx. Tests: cargo test pass."
    );
    assert!(!note.summary.contains("zephyr-quilt"));
    // Finding 1: System re-distill must not confirm its own notes, so rank stays candidate.
    store.distill_session(&person, session).await.unwrap();
    assert_eq!(column_rank(store, &note.id).await, "candidate");
    let again = store
        .list(
            &person,
            &ListFilter {
                kind: Some(Kind::Note),
                ..ListFilter::default()
            },
        )
        .await
        .unwrap()
        .memories;
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].id, note.id);
    db.finish().await;
}

#[tokio::test]
async fn handoffs_form_a_chain_and_brief_shows_latest() {
    use cairn_core::retrieve::filter::Filter;

    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let person = db.person("a").await;
    let (project, _) = store
        .init_project(&person, "github.com/acme/one", None)
        .await
        .unwrap();
    let pid = project.project_id;
    let session = |name: &str, event: &str, kind: &str, content: &str, file: Option<&str>| {
        store.capture(
            &person,
            capture(event, name, kind, content, None, Some(&pid), file),
        )
    };
    session(
        "c1",
        "c1-e",
        "edit",
        "Edit src/chain-one.rs\nx",
        Some("src/chain-one.rs"),
    )
    .await
    .unwrap();
    session("c1", "c1-p", "prompt", "TODO: first open item", None)
        .await
        .unwrap();
    store.end_session(&person, "c1").await.unwrap();
    store.distill_session(&person, "c1").await.unwrap();
    session(
        "c2",
        "c2-e",
        "edit",
        "Edit src/chain-two.rs\nx",
        Some("src/chain-two.rs"),
    )
    .await
    .unwrap();
    store.end_session(&person, "c2").await.unwrap();
    store.distill_session(&person, "c2").await.unwrap();
    session("c3", "c3-p", "prompt", "next: thin handoff later", None)
        .await
        .unwrap();
    store.end_session(&person, "c3").await.unwrap();
    let third = store.distill_session(&person, "c3").await.unwrap();
    assert!(third.handoff && third.handoff_kept);
    let rows: Vec<(String, String, serde_json::Value)> = sqlx::query_as(
        "SELECT memory_id, status, fields FROM memories
          WHERE owner_id = $1 AND kind = 'handoff' ORDER BY created_at, memory_id",
    )
    .bind(&person)
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(rows[0].1, "superseded");
    assert_eq!(rows[1].1, "superseded");
    assert_eq!(rows[2].1, "active");
    let links: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT source_id, target_id, origin FROM links
          WHERE relation = 'supersedes' ORDER BY created_at",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert_eq!(
        links,
        vec![
            (rows[1].0.clone(), rows[0].0.clone(), "system".to_owned()),
            (rows[2].0.clone(), rows[1].0.clone(), "system".to_owned()),
        ]
    );
    let auto: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM links WHERE relation = 'supersedes' AND origin = 'auto'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(
        auto, 0,
        "proof_count counts origin=auto and must skip the chain"
    );
    let work = WorkContext {
        project_id: Some(pid),
        repository: Some("github.com/acme/one".into()),
        branch: Some("feat".into()),
        worktree: Some("wt-1".into()),
        ..WorkContext::default()
    };
    let brief = store.brief(&person, &work, &[]).await.unwrap();
    assert!(
        brief.contains(&rows[1].0),
        "brief shows the richer handoff: {brief}"
    );
    assert!(
        !brief.contains(&rows[2].0),
        "brief skips the thin newest: {brief}"
    );
    let cards = store
        .search(
            &person,
            "Handoff feat",
            None,
            5,
            1200,
            &Filter {
                kind: Some(cairn_core::memory::Kind::Handoff),
                ..Filter::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(cards.results.len(), 1, "{cards:?}");
    assert_eq!(cards.results[0].id, rows[1].0);
    db.finish().await;
}

#[tokio::test]
async fn system_redistill_does_not_confirm_candidate_or_increment_proof_count() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let person = db.person("p").await;
    let (project, _) = store
        .init_project(&person, "github.com/acme/one", None)
        .await
        .unwrap();
    let pid = project.project_id;
    let session = "s-gotcha";
    store
        .capture(
            &person,
            capture(
                "g1",
                session,
                "prompt",
                "Gotcha: lock timeouts under load\nFix: retry\nReason: one database",
                None,
                Some(&pid),
                None,
            ),
        )
        .await
        .unwrap();

    // Distill 3 times
    store.distill_session(&person, session).await.unwrap();
    store.distill_session(&person, session).await.unwrap();
    store.distill_session(&person, session).await.unwrap();

    let memories = store
        .list(&person, &ListFilter::default())
        .await
        .unwrap()
        .memories;
    let gotcha = memories
        .iter()
        .find(|m| m.kind == Kind::Gotcha)
        .expect("gotcha memory exists");

    assert_eq!(column_rank(store, &gotcha.id).await, "candidate");
    assert_eq!(gotcha.proof_count, 1, "proof_count stays 1");

    let work = WorkContext {
        project_id: Some(pid),
        repository: Some("github.com/acme/one".into()),
        branch: Some("feat".into()),
        worktree: Some("wt-1".into()),
        ..WorkContext::default()
    };
    let brief = store.brief(&person, &work, &[]).await.unwrap();
    assert!(
        !brief.contains(&gotcha.id),
        "candidate gotcha must not be in the team brief: {brief}"
    );
    db.finish().await;
}

#[tokio::test]
async fn checkpoints_write_only_notes_handoff_and_digest_written_on_end_or_sweep() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let person = db.person("p").await;
    let (project, _) = store
        .init_project(&person, "github.com/acme/one", None)
        .await
        .unwrap();
    let pid = project.project_id;

    // Part 1: 3 checkpoints then end -> exactly 1 handoff and 1 digest for that session
    let s1 = "s1";
    for i in 1..=3 {
        store
            .capture(
                &person,
                capture(
                    &format!("c{i}-e"),
                    s1,
                    "edit",
                    &format!("Edit src/file{i}.rs\nx"),
                    None,
                    Some(&pid),
                    Some(&format!("src/file{i}.rs")),
                ),
            )
            .await
            .unwrap();
        store
            .capture(
                &person,
                capture(
                    &format!("c{i}-p"),
                    s1,
                    "prompt",
                    &format!("Decision: decision {i}\nReason: reason {i}\nTODO: todo {i}"),
                    None,
                    Some(&pid),
                    None,
                ),
            )
            .await
            .unwrap();
        assert!(store.checkpoint_session(&person, s1).await.unwrap());
        let rep = store.distill_session(&person, s1).await.unwrap();
        assert!(!rep.handoff, "checkpoint must not write handoff");
    }

    let mid_memories = store
        .list(&person, &ListFilter::default())
        .await
        .unwrap()
        .memories;
    let mid_handoffs = mid_memories
        .iter()
        .filter(|m| m.kind == Kind::Handoff)
        .count();
    let mid_digests = mid_memories
        .iter()
        .filter(|m| m.kind == Kind::Note && m.title.starts_with("Session "))
        .count();
    assert_eq!(mid_handoffs, 0, "0 handoffs during checkpoints");
    assert_eq!(mid_digests, 0, "0 digests during checkpoints");

    // First end_session queues the final distill
    assert!(
        store.end_session(&person, s1).await.unwrap(),
        "first end queues"
    );
    let end_rep = store.distill_session(&person, s1).await.unwrap();
    assert!(end_rep.handoff, "end distill writes handoff");

    // Repeat end_session on already-ended session queues nothing
    assert!(
        !store.end_session(&person, s1).await.unwrap(),
        "repeat end queues nothing"
    );

    let post_memories = store
        .list(&person, &ListFilter::default())
        .await
        .unwrap()
        .memories;
    let post_handoffs = post_memories
        .iter()
        .filter(|m| m.kind == Kind::Handoff)
        .count();
    let post_digests = post_memories
        .iter()
        .filter(|m| m.kind == Kind::Note && m.title.starts_with("Session "))
        .count();
    assert_eq!(post_handoffs, 1, "exactly 1 handoff after end");
    assert_eq!(post_digests, 1, "exactly 1 digest after end");

    // Part 2: checkpoints then idle -> 1 handoff + 1 digest via sweep
    let s2 = "s2";
    store
        .capture(
            &person,
            capture(
                "s2-e",
                s2,
                "edit",
                "Edit src/foo.rs\nx",
                None,
                Some(&pid),
                Some("src/foo.rs"),
            ),
        )
        .await
        .unwrap();
    store
        .capture(
            &person,
            capture(
                "s2-p",
                s2,
                "prompt",
                "next: check this",
                None,
                Some(&pid),
                None,
            ),
        )
        .await
        .unwrap();
    assert!(store.checkpoint_session(&person, s2).await.unwrap());
    store.distill_session(&person, s2).await.unwrap();

    // Age observations past idle window (make them 1 hour old, well within 48h)
    sqlx::query(
        "UPDATE observations SET captured_at = now() - interval '1 hour' WHERE session_id = $1",
    )
    .bind(s2)
    .execute(store.pool())
    .await
    .unwrap();

    let sweep_rep = store.sweep().await.unwrap();
    assert_eq!(sweep_rep.sessions_closed, 1, "sweep closed idle session s2");
    let sweep_distill = store.distill_session(&person, s2).await.unwrap();
    assert!(sweep_distill.handoff, "sweep distill writes handoff");

    // Part 3: session idle for more than 48h is NOT picked by sweep (upgrade guard)
    let s3 = "s3";
    store
        .capture(
            &person,
            capture(
                "s3-e",
                s3,
                "edit",
                "Edit src/bar.rs\nx",
                None,
                Some(&pid),
                Some("src/bar.rs"),
            ),
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE observations SET captured_at = now() - interval '50 hours' WHERE session_id = $1",
    )
    .bind(s3)
    .execute(store.pool())
    .await
    .unwrap();
    let old_sweep = store.sweep().await.unwrap();
    assert_eq!(
        old_sweep.sessions_closed, 0,
        "session older than 48h not closed by sweep"
    );

    db.finish().await;
}

#[tokio::test]
async fn thin_handoff_repeat_does_not_supersede_rich_handoff() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let person = db.person("p").await;
    let (project, _) = store
        .init_project(&person, "github.com/acme/one", None)
        .await
        .unwrap();
    let pid = project.project_id;

    // 1. Thin H1: prompt with "TODO: one item", no edits.
    let s1 = "s1";
    store
        .capture(
            &person,
            capture(
                "h1-p",
                s1,
                "prompt",
                "TODO: one item",
                None,
                Some(&pid),
                None,
            ),
        )
        .await
        .unwrap();
    store.end_session(&person, s1).await.unwrap();
    let rep1 = store.distill_session(&person, s1).await.unwrap();
    assert!(rep1.handoff);

    let h1_id = store
        .list(&person, &ListFilter::default())
        .await
        .unwrap()
        .memories
        .into_iter()
        .find(|m| m.kind == Kind::Handoff)
        .unwrap()
        .id;

    // 2. Rich H2: edit file + prompt
    let s2 = "s2";
    store
        .capture(
            &person,
            capture(
                "h2-e",
                s2,
                "edit",
                "Edit src/rich.rs\nx",
                None,
                Some(&pid),
                Some("src/rich.rs"),
            ),
        )
        .await
        .unwrap();
    store
        .capture(
            &person,
            capture(
                "h2-p",
                s2,
                "prompt",
                "TODO: next rich item",
                None,
                Some(&pid),
                None,
            ),
        )
        .await
        .unwrap();
    store.end_session(&person, s2).await.unwrap();
    let rep2 = store.distill_session(&person, s2).await.unwrap();
    assert!(rep2.handoff);

    let memories_after_h2 = store
        .list(&person, &ListFilter::default())
        .await
        .unwrap()
        .memories;
    let h2 = memories_after_h2
        .iter()
        .find(|m| m.kind == Kind::Handoff && m.id != h1_id)
        .unwrap();
    let h2_id = h2.id.clone();
    assert_eq!(h2.status, cairn_core::memory::Status::Active);

    // 3. Thin repeat same day: identical prompt content as H1, dedupes onto H1
    let s3 = "s3";
    store
        .capture(
            &person,
            capture(
                "h3-p",
                s3,
                "prompt",
                "TODO: one item",
                None,
                Some(&pid),
                None,
            ),
        )
        .await
        .unwrap();
    store.end_session(&person, s3).await.unwrap();
    let rep3 = store.distill_session(&person, s3).await.unwrap();
    // Since it deduped onto H1, report.handoff should be false
    assert!(!rep3.handoff, "deduped handoff is not written as new");

    // H2 must STILL be active, NOT superseded by H1!
    let h2_after = store.get(&person, &h2_id).await.unwrap();
    assert_eq!(
        h2_after.status,
        cairn_core::memory::Status::Active,
        "H2 must stay active and not be superseded by deduplicated H1"
    );

    // Check links: no link where H1 supersedes H2
    let all_links = links(&db).await;
    assert!(
        !all_links
            .iter()
            .any(|(src, tgt, rel, _)| src == &h1_id && tgt == &h2_id && rel == "supersedes"),
        "H1 must not supersede H2 in links: {all_links:?}"
    );

    db.finish().await;
}
