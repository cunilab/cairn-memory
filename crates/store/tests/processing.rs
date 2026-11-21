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
            "reason": "Seen in production.", "fields": {"symptom": "double charge", "fix": "Dedupe on the event id"}}))
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
    assert!(store.get(&stranger, &one).await.is_err());
    // Only the owner of the copy can forget it, and the audit trail has the promotion.
    assert!(store.forget(&stranger, global).await.is_err());
    let audit = store.list_audit(&a, 20).await.unwrap();
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
                fields = '{\"symptom\":\"x\",\"fix\":\"rotate password=hunter2hunter2 now\"}' WHERE memory_id = ANY($1)",
    )
    .bind([one.as_str(), two.as_str()])
    .execute(store.pool())
    .await
    .unwrap();
    store.consolidate_memory(&two).await.unwrap();
    let copy = globals(&db).await;
    assert_eq!(copy.len(), 1);
    let row =
        sqlx::query("SELECT summary, fields::text AS fields FROM memories WHERE memory_id = $1")
            .bind(&copy[0])
            .fetch_one(store.pool())
            .await
            .unwrap();
    let (summary, fields): (String, String) = (row.get("summary"), row.get("fields"));
    for secret in ["sk_live_abcdef123456789", "hunter2hunter2"] {
        assert!(
            !summary.contains(secret) && !fields.contains(secret),
            "{summary} {fields}"
        );
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
    let audit = store.list_audit(&b, 20).await.unwrap();
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
    sqlx::query(
        "UPDATE jobs SET finished_at = now() - interval '2 hours' WHERE status = 'quarantined'",
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
