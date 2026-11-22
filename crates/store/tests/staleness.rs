//! Step 6c: git staleness, merge promotion, distillation into the store, expiry, topic cards.

use cairn_core::capture::{ObservationInput, WorkContext};
use cairn_core::memory::{Kind, Scope, Status};
use cairn_store::memory::ListFilter;
use cairn_store::repo::RepoState;
use cairn_store::testing::{TestDb, rule};
use serde_json::json;
use sqlx::Row;

fn work(project: &str, branch: &str, files: &[&str], symbols: &[&str]) -> WorkContext {
    WorkContext {
        project_id: Some(project.to_owned()),
        repository: Some("github.com/acme/one".into()),
        branch: Some(branch.to_owned()),
        worktree: Some("wt-1".into()),
        files: files.iter().map(|f| (*f).to_owned()).collect(),
        symbols: symbols.iter().map(|s| (*s).to_owned()).collect(),
        ..WorkContext::default()
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

/// A project rule saved on `main` citing `files` and `symbols`.
async fn saved(
    db: &TestDb,
    a: &str,
    pid: &str,
    title: &str,
    files: &[&str],
    symbols: &[&str],
) -> String {
    db.store
        .save(a, rule(title), &work(pid, "main", files, symbols))
        .await
        .unwrap()
        .id
}

async fn status(db: &TestDb, a: &str, id: &str) -> (Status, Option<String>) {
    let m = db.store.get(a, id).await.unwrap();
    (m.status, m.stale_cause)
}

fn state(project: &str, branch: &str) -> RepoState {
    RepoState {
        project_id: project.to_owned(),
        branch: branch.to_owned(),
        ..RepoState::default()
    }
}

/// A project owned by `a`, with `a` already a member.
async fn project(db: &TestDb, a: &str) -> String {
    db.store
        .init_project(a, "github.com/acme/one", None)
        .await
        .unwrap()
        .0
        .project_id
}

#[tokio::test]
async fn repo_state_marks_stale_and_revives() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (store, a) = (&db.store, db.person("a").await);
    let pid = project(&db, &a).await;
    let gone = saved(&db, &a, &pid, "Queue claims use leases", &["src/a.rs"], &[]).await;
    let partly = saved(
        &db,
        &a,
        &pid,
        "Queue retries are capped",
        &["src/a.rs", "src/b.rs"],
        &[],
    )
    .await;
    let kept = saved(&db, &a, &pid, "Billing rounds to cents", &["src/c.rs"], &[]).await;
    let by_symbol = saved(
        &db,
        &a,
        &pid,
        "Leases are claimed once",
        &[],
        &["Lease::claim"],
    )
    .await;
    let unchecked = saved(&db, &a, &pid, "Reports run at midnight", &["src/z.rs"], &[]).await;

    let mut report = state(&pid, "main");
    report.files_missing = strings(&["src/a.rs"]);
    report.files_present = strings(&["src/b.rs", "src/c.rs"]);
    report.symbols_missing = strings(&["Lease::claim"]);
    let out = store.repo_state(&a, &report).await.unwrap();
    assert_eq!((out.stale, out.revived, out.promoted), (1, 0, 0));
    assert_eq!(
        status(&db, &a, &gone).await,
        (Status::Stale, Some("missing".to_owned()))
    );
    // A symbol alone never decides that a memory is gone.
    for id in [&partly, &kept, &unchecked, &by_symbol] {
        assert_eq!(status(&db, &a, id).await, (Status::Active, None), "{id}");
    }
    let checked: Vec<(String, bool)> = sqlx::query(
        "SELECT memory_id, checked_at IS NOT NULL AS checked FROM memories WHERE memory_id = ANY($1)",
    )
    .bind([&gone, &partly, &kept, &by_symbol, &unchecked])
    .fetch_all(store.pool())
    .await
    .unwrap()
    .iter()
    .map(|r| (r.get("memory_id"), r.get("checked")))
    .collect();
    for (id, was) in checked {
        assert_eq!(was, id != unchecked, "{id}");
    }

    // The file comes back: revived. A memory made stale by feedback is never revived by this.
    let mut back = state(&pid, "main");
    back.files_present = strings(&["src/a.rs"]);
    let out = store.repo_state(&a, &back).await.unwrap();
    assert_eq!((out.stale, out.revived), (0, 1));
    assert_eq!(status(&db, &a, &gone).await, (Status::Active, None));
    sqlx::query(
        "UPDATE memories SET status = 'stale', stale_cause = 'feedback' WHERE memory_id = $1",
    )
    .bind(&gone)
    .execute(store.pool())
    .await
    .unwrap();
    assert_eq!(store.repo_state(&a, &back).await.unwrap().revived, 0);
    assert_eq!(
        status(&db, &a, &gone).await,
        (Status::Stale, Some("feedback".to_owned()))
    );
    let audit = store.list_audit(&a, &Default::default()).await.unwrap();
    assert!(audit.iter().any(|e| e.action == "repo_state"));
    db.finish().await;
}

#[tokio::test]
async fn repo_state_rejects_non_default_branch() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (store, a, b) = (&db.store, db.person("a").await, db.person("b").await);
    let pid = project(&db, &a).await;
    let err = store
        .repo_state(&a, &state(&pid, "feature"))
        .await
        .expect_err("not the default branch");
    assert!(err.to_string().contains("default branch"), "{err}");
    let err = store
        .repo_state(&b, &state(&pid, "main"))
        .await
        .expect_err("not a member");
    assert!(err.to_string().contains("member"), "{err}");
    assert!(store.repo_state(&a, &state(&pid, "master")).await.is_ok());
    assert!(store.repo_state(&a, &state("ghost", "main")).await.is_err());
    db.finish().await;
}

#[tokio::test]
async fn brief_check_lists_least_checked_first_on_the_default_branch_for_members() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (store, a, b) = (&db.store, db.person("a").await, db.person("b").await);
    let pid = project(&db, &a).await;
    for (title, file) in [
        ("Queue claims use leases", "src/a.rs"),
        ("Queue retries are capped", "src/b.rs"),
        ("Billing rounds to cents", "src/c.rs"),
    ] {
        store
            .save(
                &a,
                rule(title),
                &work(&pid, "main", &[file], &["Queue::claim"]),
            )
            .await
            .unwrap();
    }
    let check = store.brief_check(&a, &pid, "main").await.unwrap().unwrap();
    assert_eq!(check.files, ["src/a.rs", "src/b.rs", "src/c.rs"]);
    assert_eq!(check.symbols, ["Queue::claim"]);
    assert_eq!(
        (check.project_id.as_str(), check.branch.as_str()),
        (pid.as_str(), "main")
    );
    // a checked file goes last
    let mut report = state(&pid, "main");
    report.files_present = strings(&["src/a.rs"]);
    store.repo_state(&a, &report).await.unwrap();
    let check = store.brief_check(&a, &pid, "main").await.unwrap().unwrap();
    assert_eq!(check.files, ["src/b.rs", "src/c.rs", "src/a.rs"]);
    // another branch, a stranger and an unknown project get none
    assert!(
        store
            .brief_check(&a, &pid, "feature")
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.brief_check(&b, &pid, "main").await.unwrap().is_none());
    assert!(
        store
            .brief_check(&a, "ghost", "main")
            .await
            .unwrap()
            .is_none()
    );
    db.finish().await;
}

fn edit_capture(
    event: &str,
    session: &str,
    branch: &str,
    file: &str,
    pid: &str,
) -> ObservationInput {
    serde_json::from_value(json!({
        "event_id": event, "session_id": session, "kind": "edit", "content": format!("Edit {file}\nx"),
        "context": {"project_id": pid, "repository": "github.com/acme/one", "branch": branch,
                    "worktree": "wt-1", "files": [file]}
    }))
    .unwrap()
}

#[tokio::test]
async fn merge_promotes_branch_and_expires_handoff() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (store, a) = (&db.store, db.person("a").await);
    let pid = project(&db, &a).await;
    let on_feature = store
        .save(
            &a,
            rule("Queue claims use leases"),
            &work(&pid, "feat", &["src/a.rs"], &[]),
        )
        .await
        .unwrap()
        .id;
    let on_other = store
        .save(
            &a,
            rule("Billing rounds to cents"),
            &work(&pid, "other", &["src/b.rs"], &[]),
        )
        .await
        .unwrap()
        .id;
    store
        .capture(&a, edit_capture("e1", "s1", "feat", "src/a.rs", &pid))
        .await
        .unwrap();
    store
        .capture(&a, edit_capture("e2", "s2", "other", "src/b.rs", &pid))
        .await
        .unwrap();
    for session in ["s1", "s2"] {
        assert!(store.distill_session(&a, session).await.unwrap().handoff);
    }
    let handoffs = |branch: &'static str| {
        let (store, a, pid) = (store, a.clone(), pid.clone());
        async move {
            sqlx::query(
                "SELECT expires_at <= now() AS expired FROM memories
                  WHERE kind = 'handoff' AND project_id = $1 AND repo_branch = $2",
            )
            .bind(&pid)
            .bind(branch)
            .fetch_one(store.pool())
            .await
            .map(|r| r.get::<bool, _>("expired"))
            .map_err(|e| format!("{e} for {a}"))
        }
    };
    let mut report = state(&pid, "main");
    // the current branch is never "merged" into itself
    report.merged_branches = strings(&["feat", "main"]);
    let out = store.repo_state(&a, &report).await.unwrap();
    assert_eq!(out.promoted, 1);
    let moved = store.get(&a, &on_feature).await.unwrap();
    assert_eq!(moved.context.branch.as_deref(), Some("main"));
    assert_eq!(
        store
            .get(&a, &on_other)
            .await
            .unwrap()
            .context
            .branch
            .as_deref(),
        Some("other")
    );
    // the merged branch's handoff is gone from reads and its row is expired; the other lives
    assert!(handoffs("feat").await.unwrap());
    assert!(!handoffs("other").await.unwrap());
    db.finish().await;
}

#[tokio::test]
async fn distill_writes_system_gotcha_fact_and_one_handoff_per_checkout() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (store, a) = (&db.store, db.person("a").await);
    let pid = project(&db, &a).await;
    let command = |event: &str, session: &str, content: &str, ok: bool| -> ObservationInput {
        serde_json::from_value(json!({
            "event_id": event, "session_id": session, "kind": "command", "ok": ok, "content": content,
            "context": {"project_id": pid, "repository": "github.com/acme/one", "branch": "feat",
                        "worktree": "wt-1", "commands": [content.lines().next().unwrap()]}
        }))
        .unwrap()
    };
    for session in ["s1", "s2"] {
        let id = |n: &str| format!("{session}-{n}");
        store
            .capture(
                &a,
                command(
                    &id("c1"),
                    session,
                    "cargo test\nexit 101: error[E0308]: mismatched types in src/billing/money.rs",
                    false,
                ),
            )
            .await
            .unwrap();
        store
            .capture(
                &a,
                edit_capture(&id("e1"), session, "feat", "src/billing/money.rs", &pid),
            )
            .await
            .unwrap();
        store
            .capture(&a, command(&id("c2"), session, "cargo test", true))
            .await
            .unwrap();
    }
    let first = store.distill_session(&a, "s1").await.unwrap();
    assert_eq!((first.notes, first.handoff), (2, true), "gotcha + fact");
    // another session of the same checkout: the same notes dedupe and the handoff is replaced
    let second = store.distill_session(&a, "s2").await.unwrap();
    assert!(second.handoff);
    let all = store
        .list(&a, &ListFilter::default())
        .await
        .unwrap()
        .memories;
    let of = |kind| all.iter().filter(|m| m.kind == kind).collect::<Vec<_>>();
    assert_eq!(
        of(Kind::Handoff).len(),
        1,
        "one handoff per repo+branch+worktree"
    );
    assert!(of(Kind::Handoff)[0].title.starts_with("Handoff: feat"));
    assert!(all.iter().all(|m| !m.title.starts_with("Session ")));
    let gotcha = of(Kind::Gotcha);
    assert_eq!(gotcha.len(), 1);
    assert_eq!(gotcha[0].scope, Scope::Personal);
    assert_eq!(
        gotcha[0].title,
        "`cargo test` failed until money.rs changed"
    );
    assert!(
        gotcha[0]
            .entities
            .iter()
            .any(|e| e.name == "src/billing/money.rs")
    );
    assert_eq!(of(Kind::Fact).len(), 1);
    // system gotcha and fact lapse after 180 days; the handoff after 14
    let days = |m: &cairn_core::memory::Memory| {
        (m.expires_at.unwrap() - cairn_core::id::now_seconds()) / 86_400
    };
    assert!(
        (179..=180).contains(&days(gotcha[0])),
        "{}",
        days(gotcha[0])
    );
    assert!((179..=180).contains(&days(of(Kind::Fact)[0])));
    assert!((13..=14).contains(&days(of(Kind::Handoff)[0])));
    // another branch of the same project gets its own handoff
    store
        .capture(&a, edit_capture("o1", "s3", "other", "src/x.rs", &pid))
        .await
        .unwrap();
    store.distill_session(&a, "s3").await.unwrap();
    let handoffs = store
        .list(&a, &ListFilter::default())
        .await
        .unwrap()
        .memories
        .into_iter()
        .filter(|m| m.kind == Kind::Handoff)
        .count();
    assert_eq!(handoffs, 2);
    db.finish().await;
}

#[tokio::test]
async fn sweep_expires_unused_stale_and_purges_distilled_observations() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (store, a) = (&db.store, db.person("a").await);
    let pid = project(&db, &a).await;
    let mut ids = Vec::new();
    for (title, file) in [
        ("Queue claims use leases", "src/a.rs"),
        ("Queue retries are capped", "src/b.rs"),
        ("Billing rounds to cents", "src/c.rs"),
    ] {
        ids.push(
            store
                .save(&a, rule(title), &work(&pid, "main", &[file], &[]))
                .await
                .unwrap()
                .id,
        );
    }
    // old+unused, old but used since going stale, recent
    for (id, age) in ids.iter().zip(["100 days", "100 days", "10 days"]) {
        sqlx::query(&format!(
            "UPDATE memories SET status = 'stale', stale_cause = 'feedback', origin = 'system',
                    status_changed_at = now() - interval '{age}' WHERE memory_id = $1"
        ))
        .bind(id)
        .execute(store.pool())
        .await
        .unwrap();
    }
    sqlx::query("UPDATE usage SET fetched = 1, last_used_at = now() WHERE memory_id = $1")
        .bind(&ids[1])
        .execute(store.pool())
        .await
        .unwrap();
    // sessions: distilled and ended 8 days ago; same but still pending; distilled 2 days ago
    for (n, (pending, ended)) in [("false", "8 days"), ("true", "8 days"), ("false", "2 days")]
        .into_iter()
        .enumerate()
    {
        let session = format!("s{n}");
        store
            .capture(
                &a,
                edit_capture(&format!("e{n}"), &session, "main", "src/a.rs", &pid),
            )
            .await
            .unwrap();
        sqlx::query(&format!(
            "UPDATE sessions SET ended_at = now() - interval '{ended}',
                    pending_since = CASE WHEN {pending} THEN now() END WHERE session_id = $1"
        ))
        .bind(&session)
        .execute(store.pool())
        .await
        .unwrap();
    }
    let report = store.sweep().await.unwrap();
    assert_eq!(report.observations_deleted, 1);
    let left: Vec<String> =
        sqlx::query_scalar("SELECT session_id FROM observations ORDER BY session_id")
            .fetch_all(store.pool())
            .await
            .unwrap();
    assert_eq!(left, ["s1", "s2"]);
    let expired: Vec<bool> = sqlx::query_scalar(
        "SELECT expires_at IS NOT NULL AND expires_at <= now() FROM memories
          WHERE memory_id = ANY($1) ORDER BY title",
    )
    .bind(&ids)
    .fetch_all(store.pool())
    .await
    .unwrap();
    // titles sort: Billing (recent), Queue claims (old, unused), Queue retries (used)
    assert_eq!(expired, [false, true, false]);
    db.finish().await;
}

#[tokio::test]
async fn fetch_extends_system_expiry_but_not_a_promoted_copy() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (store, a) = (&db.store, db.person("a").await);
    let pid = project(&db, &a).await;
    let observe = |event: &str, kind: &str, content: &str, ok: Option<bool>| -> ObservationInput {
        serde_json::from_value(json!({
            "event_id": event, "session_id": "s1", "kind": kind, "ok": ok, "content": content,
            "context": {"project_id": pid, "repository": "github.com/acme/one", "branch": "main",
                        "worktree": "wt-1", "files": ["src/a.rs"], "commands": [content.lines().next().unwrap()]}
        }))
        .unwrap()
    };
    store
        .capture(
            &a,
            observe(
                "c1",
                "command",
                "cargo test\nexit 1: error: broken beyond repair",
                Some(false),
            ),
        )
        .await
        .unwrap();
    store
        .capture(&a, observe("e1", "edit", "Edit src/a.rs\nx", None))
        .await
        .unwrap();
    store
        .capture(&a, observe("c2", "command", "cargo test", Some(true)))
        .await
        .unwrap();
    store.distill_session(&a, "s1").await.unwrap();
    let gotcha: String = sqlx::query_scalar("SELECT memory_id FROM memories WHERE kind = 'gotcha'")
        .fetch_one(store.pool())
        .await
        .unwrap();
    let expiry = |id: String| async move {
        sqlx::query_scalar::<_, f64>("SELECT EXTRACT(EPOCH FROM expires_at - now())::float8 / 86400 FROM memories WHERE memory_id = $1")
            .bind(id)
            .fetch_one(store.pool())
            .await
            .unwrap()
    };
    let shorten = |id: String| async move {
        sqlx::query(
            "UPDATE memories SET expires_at = now() + interval '10 days' WHERE memory_id = $1",
        )
        .bind(id)
        .execute(store.pool())
        .await
        .unwrap();
    };
    shorten(gotcha.clone()).await;
    assert!(expiry(gotcha.clone()).await < 11.0);
    store
        .mark_fetched(std::slice::from_ref(&gotcha))
        .await
        .unwrap();
    assert!(
        expiry(gotcha.clone()).await > 179.0,
        "a fetch restarts the 180 days"
    );
    // a helpful vote does too
    shorten(gotcha.clone()).await;
    store
        .feedback(&a, &gotcha, cairn_store::memory::Vote::Helpful)
        .await
        .unwrap();
    assert!(expiry(gotcha.clone()).await > 179.0);
    // an agent memory and a promoted global copy are left alone
    let rule_id = store
        .save(
            &a,
            rule("Queue claims use leases"),
            &work(&pid, "main", &[], &[]),
        )
        .await
        .unwrap()
        .id;
    sqlx::query("UPDATE memories SET origin = 'system', scope = 'global', project_id = NULL, expires_at = NULL WHERE memory_id = $1")
        .bind(&rule_id)
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE memories SET kind = 'gotcha' WHERE memory_id = $1")
        .bind(&rule_id)
        .execute(store.pool())
        .await
        .unwrap();
    store
        .mark_fetched(std::slice::from_ref(&rule_id))
        .await
        .unwrap();
    let still_none: Option<f64> = sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM expires_at)::float8 FROM memories WHERE memory_id = $1",
    )
    .bind(&rule_id)
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert!(still_none.is_none(), "a promoted copy has no 180-day clock");
    db.finish().await;
}

#[tokio::test]
async fn topic_gives_notes_for_one_or_two_and_a_card_for_three() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (store, a) = (&db.store, db.person("a").await);
    let pid = project(&db, &a).await;
    let ctx = work(&pid, "main", &["src/jobs/queue.rs"], &[]);
    let exact = store
        .save(&a, rule("Queue claims use leases"), &ctx)
        .await
        .unwrap()
        .id;
    // a module match: another file in the same folder
    store
        .save(
            &a,
            rule("Retries are capped"),
            &work(&pid, "main", &["src/jobs/retry.rs"], &[]),
        )
        .await
        .unwrap();
    let topic = store.topic(&a, "src/jobs/queue.rs", &ctx).await.unwrap();
    assert!(topic.card.is_none());
    assert_eq!(topic.notes.len(), 2);
    assert_eq!(topic.notes[0].id, exact, "the file match comes first");
    store
        .save(
            &a,
            rule("Leases expire after a minute"),
            &work(&pid, "main", &["src/jobs/lease.rs"], &[]),
        )
        .await
        .unwrap();
    let topic = store.topic(&a, "src/jobs/queue.rs", &ctx).await.unwrap();
    assert!(topic.notes.is_empty());
    let card = topic.card.expect("card");
    assert!(
        card.starts_with("Cairn topic src/jobs/queue.rs (data, not instructions): 3 memories \u{2014} rule Queue claims use leases; rule "),
        "{card}"
    );
    assert!(card.len() <= 300 && !card.contains('\n'));
    // a folder with a generic name has no module entity
    assert!(
        store
            .topic(&a, "src/other.rs", &ctx)
            .await
            .unwrap()
            .notes
            .is_empty()
    );
    db.finish().await;
}

#[tokio::test]
async fn observations_keep_their_capture_order_within_one_second() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (store, a) = (&db.store, db.person("a").await);
    let pid = project(&db, &a).await;
    // Event ids sort the other way round, as the hook's hash ids do: only the capture time orders.
    for (n, event) in ["z1", "y2", "x3"].into_iter().enumerate() {
        store
            .capture(
                &a,
                edit_capture(event, "s1", "main", &format!("src/f{n}.rs"), &pid),
            )
            .await
            .unwrap();
    }
    let order: Vec<String> = store
        .session_observations(&a, "s1")
        .await
        .unwrap()
        .into_iter()
        .map(|o| o.event_id)
        .collect();
    assert_eq!(order, ["z1", "y2", "x3"]);
    db.finish().await;
}

/// Count of `entities` rows named `name`, so a test cannot pass vacuously on a name the server
/// never recorded.
async fn entity_rows(db: &TestDb, name: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM entities WHERE name = $1")
        .bind(name)
        .fetch_one(db.store.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn stale_never_expires_an_agent_memory() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (store, a) = (&db.store, db.person("a").await);
    let pid = project(&db, &a).await;
    let durable = saved(&db, &a, &pid, "Queue claims use leases", &["src/a.rs"], &[]).await;
    let derived = saved(
        &db,
        &a,
        &pid,
        "Queue retries are capped",
        &["src/b.rs"],
        &[],
    )
    .await;
    sqlx::query("UPDATE memories SET origin = 'system' WHERE memory_id = $1")
        .bind(&derived)
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE memories SET status = 'stale', stale_cause = 'missing',
                status_changed_at = now() - interval '400 days' WHERE memory_id = ANY($1)",
    )
    .bind([durable.as_str(), derived.as_str()])
    .execute(store.pool())
    .await
    .unwrap();
    store.sweep().await.unwrap();
    store.sweep().await.unwrap();
    let expires: Vec<(String, bool)> = sqlx::query(
        "SELECT memory_id, expires_at IS NOT NULL AS expires FROM memories WHERE memory_id = ANY($1)",
    )
    .bind([durable.as_str(), derived.as_str()])
    .fetch_all(store.pool())
    .await
    .unwrap()
    .iter()
    .map(|r| (r.get("memory_id"), r.get("expires")))
    .collect();
    for (id, expires) in expires {
        // Only system-derived rows lapse; an agent's memory stays, stale and down-ranked.
        assert_eq!(expires, id == derived, "{id}");
    }
    assert_eq!(status(&db, &a, &durable).await.0, Status::Stale);
    db.finish().await;
}

#[tokio::test]
async fn repo_state_marks_stale_only_when_sure_the_file_is_gone() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (store, a) = (&db.store, db.person("a").await);
    let pid = project(&db, &a).await;
    let control = saved(
        &db,
        &a,
        &pid,
        "Queue claims use leases",
        &["src/gone.rs"],
        &[],
    )
    .await;
    let absolute = saved(
        &db,
        &a,
        &pid,
        "Nginx lives in etc",
        &["/etc/nginx/nginx.conf"],
        &[],
    )
    .await;
    let outside = saved(
        &db,
        &a,
        &pid,
        "Sibling repo readme",
        &["../other/README.md"],
        &[],
    )
    .await;
    let words = saved(&db, &a, &pid, "Keep user id snake case", &[], &["user_id"]).await;
    let branch_only = store
        .save(
            &a,
            rule("Feature branch only file"),
            &work(&pid, "feat", &["src/feature.rs"], &[]),
        )
        .await
        .unwrap()
        .id;
    for name in [
        "/etc/nginx/nginx.conf",
        "../other/README.md",
        "user_id",
        "src/feature.rs",
    ] {
        assert!(entity_rows(&db, name).await > 0, "{name} was not recorded");
    }
    let mut report = state(&pid, "main");
    report.files_missing = strings(&[
        "src/gone.rs",
        "/etc/nginx/nginx.conf",
        "../other/README.md",
        "src/feature.rs",
    ]);
    report.symbols_missing = strings(&["user_id"]);
    let out = store.repo_state(&a, &report).await.unwrap();
    assert_eq!(out.stale, 1);
    assert_eq!(status(&db, &a, &control).await.0, Status::Stale);
    for id in [&absolute, &outside, &words, &branch_only] {
        assert_eq!(status(&db, &a, id).await.0, Status::Active, "{id}");
    }
    db.finish().await;
}

#[tokio::test]
async fn repo_state_never_rewrites_another_members_branches_or_handoffs() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (store, a, b) = (&db.store, db.person("a").await, db.person("b").await);
    let pid = project(&db, &a).await;
    store.join_project(&b, &pid, None).await.unwrap();
    let mine = saved_on(&db, &a, &pid, "Queue claims use leases", "dev").await;
    let theirs = saved_on(&db, &b, &pid, "Billing rounds to cents", "dev").await;
    store
        .capture(&a, edit_capture("e1", "sa", "dev", "src/a.rs", &pid))
        .await
        .unwrap();
    store
        .capture(&b, edit_capture("e2", "sb", "dev", "src/b.rs", &pid))
        .await
        .unwrap();
    assert!(store.distill_session(&a, "sa").await.unwrap().handoff);
    assert!(store.distill_session(&b, "sb").await.unwrap().handoff);
    let mut report = state(&pid, "main");
    report.merged_branches = strings(&["dev"]);
    let out = store.repo_state(&a, &report).await.unwrap();
    assert_eq!(out.promoted, 1, "only the reporter's own memory moves");
    let branch = |id: String| {
        let store = store.clone();
        async move {
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT repo_branch FROM memories WHERE memory_id = $1",
            )
            .bind(id)
            .fetch_one(store.pool())
            .await
            .unwrap()
        }
    };
    assert_eq!(branch(mine).await.as_deref(), Some("main"));
    assert_eq!(branch(theirs).await.as_deref(), Some("dev"));
    let expired: Vec<(String, bool)> = sqlx::query(
        "SELECT owner_id, expires_at <= now() AS expired FROM memories WHERE kind = 'handoff'",
    )
    .fetch_all(store.pool())
    .await
    .unwrap()
    .iter()
    .map(|r| (r.get("owner_id"), r.get("expired")))
    .collect();
    assert_eq!(expired.len(), 2);
    for (owner, expired) in expired {
        assert_eq!(expired, owner == a, "only the reporter's handoff expires");
    }
    db.finish().await;
}

async fn saved_on(db: &TestDb, who: &str, pid: &str, title: &str, branch: &str) -> String {
    db.store
        .save(who, rule(title), &work(pid, branch, &["src/x.rs"], &[]))
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn a_forgotten_handoff_does_not_block_the_checkout() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (store, a) = (&db.store, db.person("a").await);
    let pid = project(&db, &a).await;
    store
        .capture(&a, edit_capture("e1", "s1", "feat", "src/a.rs", &pid))
        .await
        .unwrap();
    assert!(store.distill_session(&a, "s1").await.unwrap().handoff);
    let handoff: String =
        sqlx::query_scalar("SELECT memory_id FROM memories WHERE kind = 'handoff'")
            .fetch_one(store.pool())
            .await
            .unwrap();
    store.forget(&a, &handoff).await.unwrap();
    store
        .capture(&a, edit_capture("e2", "s2", "feat", "src/b.rs", &pid))
        .await
        .unwrap();
    assert!(
        store.distill_session(&a, "s2").await.unwrap().handoff,
        "forgetting one handoff is not forgetting the checkout"
    );
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memories WHERE kind = 'handoff' AND status = 'active'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(live, 1);
    db.finish().await;
}
