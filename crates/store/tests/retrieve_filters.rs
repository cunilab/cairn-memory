//! Candidate-pool behaviour at a size where the pool matters: the request filter must act before
//! the pool is cut (I1), a context entity must not pass the gate (I2), and the brief must keep
//! its handoff, preferences and file-matched rules however many newer rules exist (I3).

use cairn_core::capture::WorkContext;
use cairn_core::memory::Kind;
use cairn_core::retrieve::filter::Filter;
use cairn_store::testing::{TestDb, save_input};

/// `rows` project memories `<prefix>1..` of one kind, all with the same text, created from
/// `future_seconds` from now on (negative: in the past), one second apart.
#[allow(clippy::too_many_arguments)]
async fn bulk(
    db: &TestDb,
    owner: &str,
    project: &str,
    prefix: &str,
    kind: &str,
    title: &str,
    rows: i32,
    future_seconds: i32,
) {
    sqlx::query(
        "INSERT INTO memories (memory_id, owner_id, project_id, scope, kind, title, summary, reason,
                               fields, evidence, evidence_hash, identity_hash, created_at, updated_at)
         SELECT $1 || n, $2, $3, 'project', $4, $5 || ' ' || n, 'plain summary text', 'a reason',
                '{}'::jsonb, ARRAY[]::text[], 'e', $1 || 'i' || n,
                now() + ($7 + n) * interval '1 second', now() + ($7 + n) * interval '1 second'
           FROM generate_series(1, $6) n",
    )
    .bind(prefix)
    .bind(owner)
    .bind(project)
    .bind(kind)
    .bind(title)
    .bind(rows)
    .bind(future_seconds)
    .execute(db.store.pool())
    .await
    .unwrap();
    sqlx::raw_sql("ANALYZE memories")
        .execute(db.store.pool())
        .await
        .unwrap();
}

fn in_project(project: &str) -> Filter {
    Filter {
        context: Some(WorkContext {
            project_id: Some(project.to_owned()),
            ..WorkContext::default()
        }),
        ..Filter::default()
    }
}

#[tokio::test]
async fn project_and_kind_filter_act_before_the_candidate_pool() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let (a, _) = store
        .init_project(&me, "github.com/acme/a", None)
        .await
        .unwrap();
    let (b, _) = store
        .init_project(&me, "github.com/acme/b", None)
        .await
        .unwrap();
    // Project B owns the newest 400 rows with the same words, so an unfiltered channel fills its
    // slots with B; project A's 400 rows come after them.
    bulk(
        &db,
        &me,
        &a.project_id,
        "a",
        "fact",
        "gateway rollout",
        400,
        0,
    )
    .await;
    bulk(
        &db,
        &me,
        &b.project_id,
        "b",
        "fact",
        "gateway rollout",
        400,
        1000,
    )
    .await;

    let cards = store
        .search(
            &me,
            "gateway rollout",
            None,
            5,
            4000,
            &in_project(&a.project_id),
        )
        .await
        .unwrap();
    assert_eq!(
        cards.results.len(),
        5,
        "the limit is reached from project A alone"
    );
    assert!(
        cards.results.iter().all(|c| c.id.starts_with('a')),
        "only project A's memories: {:?}",
        cards.results.iter().map(|c| &c.id).collect::<Vec<_>>()
    );

    // The kind filter works the same way: 3 old rules under 400 newer facts.
    bulk(
        &db,
        &me,
        &a.project_id,
        "r",
        "rule",
        "gateway rollout",
        3,
        -5000,
    )
    .await;
    bulk(
        &db,
        &me,
        &a.project_id,
        "f",
        "fact",
        "gateway rollout",
        400,
        2000,
    )
    .await;
    let only_rules = Filter {
        kind: Some(Kind::Rule),
        ..in_project(&a.project_id)
    };
    let cards = store
        .search(&me, "gateway rollout", None, 5, 4000, &only_rules)
        .await
        .unwrap();
    let mut ids: Vec<&str> = cards.results.iter().map(|c| c.id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, ["r1", "r2", "r3"]);
    db.finish().await;
}

#[tokio::test]
async fn a_context_entity_does_not_pass_the_gate() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let work = WorkContext {
        files: vec!["src/payments/webhooks.rs".into()],
        ..WorkContext::default()
    };
    store
        .save(
            &me,
            save_input(serde_json::json!({"kind":"fact","title":"Webhook retries","summary":"Providers resend callbacks.","reason":"r and the test says why"})),
            &work,
        )
        .await
        .unwrap();
    let editing = Filter {
        context: Some(work.clone()),
        ..Filter::default()
    };
    let unrelated = store
        .search(&me, "volcanic eruption evacuation", None, 5, 1200, &editing)
        .await
        .unwrap();
    assert!(
        unrelated.results.is_empty(),
        "an unrelated query gets no card just because a file is open: {unrelated:?}"
    );
    // A path the query itself names still counts.
    let named = store
        .search(
            &me,
            "src/payments/webhooks.rs",
            None,
            5,
            1200,
            &Filter::default(),
        )
        .await
        .unwrap();
    assert_eq!(named.results.len(), 1);
    db.finish().await;
}

#[tokio::test]
async fn brief_keeps_handoff_preferences_and_file_rules_under_newer_rules() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let (project, _) = store
        .init_project(&me, "github.com/acme/brief", None)
        .await
        .unwrap();
    let ctx = WorkContext {
        project_id: Some(project.project_id.clone()),
        repository: Some("github.com/acme/brief".into()),
        branch: Some("main".into()),
        files: vec!["src/jobs/queue.rs".into()],
        ..WorkContext::default()
    };
    let save = |value: serde_json::Value| {
        let ctx = ctx.clone();
        let me = me.clone();
        async move { store.save(&me, save_input(value), &ctx).await.unwrap().id }
    };
    // Handoffs are written by the system only, so insert one the way capture would.
    let handoff = "handoff_probe".to_owned();
    sqlx::query(
        "INSERT INTO memories (memory_id, owner_id, project_id, scope, kind, title, summary, reason,
                               fields, evidence, evidence_hash, identity_hash, repo, repo_branch)
         VALUES ($1, $2, $3, 'personal', 'handoff', 'Where I stopped', 'Queue retry work is half done.',
                 'r', '{}'::jsonb, ARRAY[]::text[], 'e', 'hi', 'github.com/acme/brief', 'main')",
    )
    .bind(&handoff)
    .bind(&me)
    .bind(&project.project_id)
    .execute(store.pool())
    .await
    .unwrap();
    let preference = save(serde_json::json!({"kind":"preference","scope":"personal","title":"Terse replies","summary":"Keep answers short. is a summary of the test","reason":"r and the test says why"})).await;
    let file_rule = save(serde_json::json!({"kind":"rule","scope":"project","title":"Queue retries are capped","summary":"Never retry a job more than five times.","reason":"r and the test says why"})).await;
    // 350 newer rules with no file: the old single `LIMIT 300` query never saw the three above.
    bulk(
        &db,
        &me,
        &project.project_id,
        "n",
        "rule",
        "unrelated convention",
        350,
        100,
    )
    .await;

    let brief = store
        .brief(&me, &ctx, &["src/jobs/queue.rs".to_owned()], None)
        .await
        .unwrap()
        .text;
    for (what, id) in [
        ("handoff", &handoff),
        ("preference", &preference),
        ("file rule", &file_rule),
    ] {
        assert!(
            brief.contains(id.as_str()),
            "{what} is missing from the brief: {brief}"
        );
    }
    db.finish().await;
}
