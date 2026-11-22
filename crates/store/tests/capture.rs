use cairn_core::capture::{ObservationInput, WorkContext};
use cairn_core::memory::{Kind, Scope};
use cairn_core::process::jobs::{distill_key, is_empty_session};
use cairn_store::StoreError;
use cairn_store::memory::ListFilter;
use cairn_store::testing::TestDb;

fn observation(event: &str, session: &str, content: &str) -> ObservationInput {
    serde_json::from_value(serde_json::json!({
        "event_id": event, "session_id": session, "content": content, "kind": "assistant",
        "context": {"repository": "github.com/acme/cairn", "branch": "main", "files": ["src/a.rs"]}
    }))
    .unwrap()
}

fn prompt(event: &str, session: &str, content: &str) -> ObservationInput {
    let mut input = observation(event, session, content);
    input.kind = cairn_core::capture::CaptureKind::Prompt;
    input
}

#[tokio::test]
async fn capture_is_idempotent_and_changed_content_is_409() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let first = db
        .store
        .capture(
            &me,
            observation("e1", "s1", "api_key=do-not-store was seen"),
        )
        .await
        .unwrap();
    assert!(first.accepted && !first.duplicate);
    let replay = db
        .store
        .capture(
            &me,
            observation("e1", "s1", "api_key=do-not-store was seen"),
        )
        .await
        .unwrap();
    assert!(replay.duplicate);

    let changed = db
        .store
        .capture(&me, observation("e1", "s1", "something else"))
        .await;
    assert!(
        matches!(changed, Err(StoreError::Conflict(_))),
        "{changed:?}"
    );
    let other_session = db
        .store
        .capture(
            &me,
            observation("e1", "s2", "api_key=do-not-store was seen"),
        )
        .await;
    assert!(matches!(other_session, Err(StoreError::Conflict(_))));

    let stored = db.store.session_observations(&me, "s1").await.unwrap();
    assert_eq!(stored.len(), 1);
    assert!(!stored[0].content.contains("do-not-store"));
    assert_eq!(stored[0].expires_at - stored[0].captured_at, 30 * 86_400);

    // The event id belongs to the person: someone else may use the same id.
    let other = db.person("other").await;
    assert!(
        !db.store
            .capture(&other, observation("e1", "s1", "mine"))
            .await
            .unwrap()
            .duplicate
    );
    assert!(
        db.store
            .capture(&me, observation(" ", "s1", "x"))
            .await
            .is_err()
    );
    db.finish().await;
}

#[tokio::test]
async fn bare_project_context_inherits_the_session_context() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let first: ObservationInput = serde_json::from_value(serde_json::json!({
        "event_id": "e0", "session_id": "s1", "content": "x", "kind": "prompt",
        "context": {"branch": "feature"}
    }))
    .unwrap();
    db.store.capture(&me, first).await.unwrap();
    let bare: ObservationInput = serde_json::from_value(serde_json::json!({
        "event_id": "e1", "session_id": "s1", "content": "y", "kind": "prompt"
    }))
    .unwrap();
    db.store.capture(&me, bare).await.unwrap();
    let stored = db.store.session_observations(&me, "s1").await.unwrap();
    assert_eq!(stored[0].context.branch.as_deref(), Some("feature"));
    db.finish().await;
}

#[tokio::test]
async fn ending_a_session_queues_distill_and_distill_writes_notes_and_one_handoff() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    assert!(
        !db.store.end_session(&me, "never-seen").await.unwrap(),
        "nothing to distill"
    );

    db.store
        .capture(
            &me,
            prompt(
                "e1",
                "s1",
                "Decision: use pgvector\nReason: one database\nTODO: wire the worker",
            ),
        )
        .await
        .unwrap();
    assert!(db.store.checkpoint_session(&me, "s1").await.unwrap());
    assert!(
        db.store.end_session(&me, "s1").await.unwrap(),
        "end queues again, still one pending job"
    );
    let jobs = db
        .store
        .claim_jobs(cairn_core::process::jobs::JobKind::Distill, 10)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].key, distill_key(&me, "s1"));

    let report = db.store.distill_session(&me, "s1").await.unwrap();
    assert_eq!((report.notes, report.handoff), (1, true));
    let all = db
        .store
        .list(&me, &ListFilter::default())
        .await
        .unwrap()
        .memories;
    let decision = all
        .iter()
        .find(|m| m.kind == Kind::Decision)
        .expect("typed note");
    assert_eq!(
        (decision.scope, decision.reason.as_str()),
        (Scope::Personal, "one database")
    );
    let handoff = all
        .iter()
        .find(|m| m.kind == Kind::Handoff)
        .expect("handoff");
    assert!(handoff.expires_at.is_none());
    assert_eq!(
        handoff.fields.to_json()["next_steps"][0],
        "TODO: wire the worker"
    );

    // Distilling the same captures again does not add a second handoff.
    let again = db.store.distill_session(&me, "s1").await.unwrap();
    assert!(!again.handoff);
    let all = db
        .store
        .list(&me, &ListFilter::default())
        .await
        .unwrap()
        .memories;
    assert_eq!(all.iter().filter(|m| m.kind == Kind::Handoff).count(), 1);
    assert_eq!(all.iter().filter(|m| m.kind == Kind::Decision).count(), 1);

    let empty = db
        .store
        .distill_session(&me, "ghost")
        .await
        .expect_err("nothing retained");
    assert!(is_empty_session(&empty.to_string()), "{empty}");

    // A forgotten handoff is not resurrected.
    db.store.forget(&me, &handoff.id).await.unwrap();
    assert!(!db.store.distill_session(&me, "s1").await.unwrap().handoff);
    db.finish().await;
}

#[tokio::test]
async fn sweep_closes_idle_sessions_and_deletes_old_rows() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    db.store
        .capture(&me, observation("e1", "idle", "TODO: later"))
        .await
        .unwrap();
    db.store
        .capture(&me, observation("e2", "busy", "TODO: now"))
        .await
        .unwrap();
    sqlx::query("UPDATE observations SET captured_at = now() - interval '2 hours' WHERE session_id = 'idle'")
        .execute(db.store.pool())
        .await
        .unwrap();
    let note = db
        .store
        .save(
            &me,
            cairn_store::testing::save_input(
                serde_json::json!({"kind":"note","title":"old title for tests","summary":"s is a summary of the test"}),
            ),
            &WorkContext::default(),
        )
        .await
        .unwrap();
    sqlx::query("UPDATE memories SET expires_at = now() - interval '31 days' WHERE memory_id = $1")
        .bind(&note.id)
        .execute(db.store.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE observations SET expires_at = now() - interval '1 day' WHERE session_id = 'busy'",
    )
    .execute(db.store.pool())
    .await
    .unwrap();

    let report = db.store.sweep().await.unwrap();
    assert_eq!(
        (
            report.sessions_closed,
            report.memories_deleted,
            report.observations_deleted
        ),
        (1, 0, 1)
    );
    let jobs = db
        .store
        .claim_jobs(cairn_core::process::jobs::JobKind::Distill, 10)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].key, distill_key(&me, "idle"));
    assert_eq!(
        db.store.sweep().await.unwrap().sessions_closed,
        0,
        "closed sessions are not closed twice"
    );
    db.finish().await;
}

/// A memory far past `expires_at` is still there after sweep, and a new note has no TTL.
#[tokio::test]
async fn sweep_never_deletes_or_expires_memories() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let work = WorkContext::default();
    let kinds = [
        ("decision", true),
        ("rule", true),
        ("gotcha", true),
        ("procedure", true),
        ("fact", true),
        ("preference", true),
        ("note", false),
    ];
    for (kind, needs_reason) in kinds {
        let mut body = serde_json::json!({
            "kind": kind,
            "title": format!("{kind} title for tests"),
            "summary": format!("{kind} summary is long enough for the test"),
        });
        if needs_reason {
            body["reason"] = serde_json::json!("because the test says why");
        }
        if kind == "gotcha" {
            body["fields"] = serde_json::json!({
                "symptom": "the client hangs",
                "cause": "no timeout",
                "fix": "set the timeout explicitly in the client"
            });
        }
        if kind == "procedure" {
            body["fields"] = serde_json::json!({"steps": ["open the gate", "close the gate"]});
        }
        let saved = db
            .store
            .save(&me, cairn_store::testing::save_input(body), &work)
            .await
            .unwrap();
        let memory = db.store.get(&me, &saved.id).await.unwrap();
        assert!(
            memory.expires_at.is_none(),
            "{kind} expires_at = {:?}",
            memory.expires_at
        );
    }
    let old = db
        .store
        .save(
            &me,
            cairn_store::testing::save_input(serde_json::json!({
                "kind": "fact",
                "title": "Ancient fact title",
                "summary": "Ancient fact summary is long enough",
                "reason": "because the test says why"
            })),
            &work,
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE memories SET expires_at = now() - interval '400 days' WHERE memory_id = $1",
    )
    .bind(&old.id)
    .execute(db.store.pool())
    .await
    .unwrap();
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM memories")
        .fetch_one(db.store.pool())
        .await
        .unwrap();
    let report = db.store.sweep().await.unwrap();
    assert_eq!(report.memories_deleted, 0);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM memories")
        .fetch_one(db.store.pool())
        .await
        .unwrap();
    assert_eq!(after, before);
    let still = db.store.get(&me, &old.id).await.expect("expired row stays");
    assert_eq!(still.title, "Ancient fact title");
    let found = db
        .store
        .search(
            &me,
            "Ancient fact title",
            None,
            5,
            1200,
            &cairn_core::retrieve::filter::Filter::default(),
        )
        .await
        .unwrap();
    assert!(
        found.results.iter().any(|card| card.id == old.id),
        "lexical search still returns it: {found:?}"
    );
    db.finish().await;
}

#[tokio::test]
async fn withdrawn_promoted_copy_stays_active() {
    use cairn_core::memory::Status;

    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let (project, _) = db
        .store
        .init_project(&me, "github.com/acme/one", None)
        .await
        .unwrap();
    let work = WorkContext {
        project_id: Some(project.project_id),
        ..WorkContext::default()
    };
    let one = db
        .store
        .save(&me, cairn_store::testing::rule("Source one rule"), &work)
        .await
        .unwrap()
        .id;
    let two = db
        .store
        .save(&me, cairn_store::testing::rule("Source two rule"), &work)
        .await
        .unwrap()
        .id;
    sqlx::query(
        "INSERT INTO memories (memory_id, owner_id, scope, kind, title, summary, reason, evidence_hash, identity_hash, origin)
         VALUES ('mem_global_copy', $1, 'global', 'rule', 'Global copy title', 'Global copy summary is long enough', 'because the test says why', 'eg', 'ig', 'system')",
    )
    .bind(&me)
    .execute(db.store.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO links (source_id, target_id, relation, origin) VALUES
           ('mem_global_copy', $1, 'relates', 'auto'),
           ('mem_global_copy', $2, 'relates', 'auto')",
    )
    .bind(&one)
    .bind(&two)
    .execute(db.store.pool())
    .await
    .unwrap();
    db.store.forget(&me, &one).await.unwrap();
    assert_eq!(db.store.reconcile_promoted().await.unwrap(), (0, 0));
    let copy = db.store.get(&me, "mem_global_copy").await.unwrap();
    assert_eq!(copy.status, Status::Active);
    let links: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM links WHERE source_id = 'mem_global_copy' AND relation = 'relates'",
    )
    .fetch_one(db.store.pool())
    .await
    .unwrap();
    assert_eq!(links, 2);
    db.finish().await;
}

#[tokio::test]
async fn same_session_same_content_is_duplicate() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let first = db
        .store
        .capture(&me, observation("e1", "s1", "cargo test"))
        .await
        .unwrap();
    assert!(!first.duplicate);
    // A new event id with the same content in the same session writes nothing.
    let again = db
        .store
        .capture(&me, observation("e2", "s1", "cargo test"))
        .await
        .unwrap();
    assert!(again.accepted && again.duplicate);
    assert_eq!(again.event_id, "e2");
    assert_eq!(
        db.store
            .session_observations(&me, "s1")
            .await
            .unwrap()
            .len(),
        1
    );
    // Another session, another kind, or another person is not a duplicate.
    assert!(
        !db.store
            .capture(&me, observation("e3", "s2", "cargo test"))
            .await
            .unwrap()
            .duplicate
    );
    let mut command = observation("e4", "s1", "cargo test");
    command.kind = cairn_core::capture::CaptureKind::Command;
    assert!(!db.store.capture(&me, command).await.unwrap().duplicate);
    let other = db.person("other").await;
    assert!(
        !db.store
            .capture(&other, observation("e5", "s1", "cargo test"))
            .await
            .unwrap()
            .duplicate
    );
    db.finish().await;
}

#[tokio::test]
async fn kind_ok_role_and_source_are_stored() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let mut input = observation("e1", "s1", "cargo test\nexit 101: boom");
    input.kind = cairn_core::capture::CaptureKind::Command;
    input.ok = Some(false);
    db.store.capture(&me, input).await.unwrap();
    db.store
        .capture(&me, observation("e2", "s1", "done"))
        .await
        .unwrap();
    let stored = db.store.session_observations(&me, "s1").await.unwrap();
    let key = |i: usize| {
        let o = &stored[i];
        (o.kind.as_str(), o.ok, o.role.as_str(), o.source.as_str())
    };
    assert_eq!(key(0), ("command", Some(false), "tool", "command"));
    assert_eq!(key(1), ("assistant", None, "assistant", "assistant"));
    db.finish().await;
}

#[tokio::test]
async fn brief_session_keeps_recent_files_across_captures() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let work = WorkContext {
        repository: Some("github.com/acme/cairn".into()),
        branch: Some("main".into()),
        ..WorkContext::default()
    };
    let files = vec!["src/a.rs".to_owned(), "password=hunter2".to_owned()];
    db.store
        .record_session_files(&me, "s1", &work, &files)
        .await
        .unwrap();
    db.store
        .capture(&me, observation("e1", "s1", "work"))
        .await
        .unwrap();
    let context: serde_json::Value = sqlx::query_scalar(
        "SELECT context FROM sessions WHERE person_id = $1 AND session_id = 's1'",
    )
    .bind(&me)
    .fetch_one(db.store.pool())
    .await
    .unwrap();
    assert_eq!(context["recent_files"][0], "src/a.rs");
    assert!(
        !context["recent_files"][1]
            .as_str()
            .unwrap()
            .contains("hunter2")
    );
    assert_eq!(context["branch"], "main");
    // A session opened by the brief has nothing to distill.
    assert!(!db.store.end_session(&me, "unused").await.unwrap());
    db.finish().await;
}

async fn session_project(db: &TestDb, person: &str, session: &str) -> Option<String> {
    sqlx::query_scalar("SELECT project_id FROM sessions WHERE person_id = $1 AND session_id = $2")
        .bind(person)
        .bind(session)
        .fetch_one(db.store.pool())
        .await
        .unwrap()
}

fn repo_observation(event: &str, session: &str, repository: &str) -> ObservationInput {
    serde_json::from_value(serde_json::json!({
        "event_id": event,
        "session_id": session,
        "content": format!("captured {event}"),
        "kind": "assistant",
        "context": {"repository": repository, "branch": "main"}
    }))
    .unwrap()
}

/// One member project for the repository attaches; two, or none, stays personal.
#[tokio::test]
async fn repository_fallback_needs_exactly_one_membership() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let repo = "github.com/acme/widgets";
    let owner_a = db.person("owner-a").await;
    let owner_b = db.person("owner-b").await;
    let member = db.person("member").await;
    let stranger = db.person("stranger").await;
    let (first, _) = db
        .store
        .init_project(&owner_a, repo, Some("Widgets"))
        .await
        .unwrap();
    let (second, _) = db
        .store
        .init_project(&owner_b, repo, Some("Fork"))
        .await
        .unwrap();
    db.store
        .join_project(&member, &first.project_id, None)
        .await
        .unwrap();

    db.store
        .capture(&member, repo_observation("one", "s-one", repo))
        .await
        .unwrap();
    assert_eq!(
        session_project(&db, &member, "s-one").await.as_deref(),
        Some(first.project_id.as_str()),
        "exactly one membership attaches"
    );
    // The case the personal-session fixture must not hit: no project id, but this repository
    // is the person's only project, so the session and its distilled note attach.
    let mut decision = repo_observation("decide", "s-decide", repo);
    decision.content =
        "Decision: keep the queue in Postgres\nReason: one database is enough here".to_owned();
    db.store.capture(&member, decision).await.unwrap();
    assert!(
        db.store
            .distill_session(&member, "s-decide")
            .await
            .unwrap()
            .notes
            >= 1
    );
    let (note_project, rank): (Option<String>, String) = sqlx::query_as(
        "SELECT project_id, rank FROM memories WHERE owner_id = $1 AND kind = 'decision'",
    )
    .bind(&member)
    .fetch_one(db.store.pool())
    .await
    .unwrap();
    assert_eq!(
        (note_project.as_deref(), rank.as_str()),
        (Some(first.project_id.as_str()), "candidate")
    );
    let saved = db
        .store
        .save(
            &member,
            cairn_store::testing::rule("Repository fallback save"),
            &WorkContext {
                repository: Some(repo.into()),
                ..WorkContext::default()
            },
        )
        .await
        .unwrap();
    let saved_project: Option<String> =
        sqlx::query_scalar("SELECT project_id FROM memories WHERE memory_id = $1")
            .bind(&saved.id)
            .fetch_one(db.store.pool())
            .await
            .unwrap();
    assert_eq!(saved_project.as_deref(), Some(first.project_id.as_str()));

    db.store
        .join_project(&member, &second.project_id, None)
        .await
        .unwrap();
    db.store
        .capture(&member, repo_observation("two", "s-two", repo))
        .await
        .unwrap();
    assert_eq!(
        session_project(&db, &member, "s-two").await,
        None,
        "two memberships stay personal"
    );

    db.store
        .capture(&stranger, repo_observation("out", "s-out", repo))
        .await
        .unwrap();
    assert_eq!(session_project(&db, &stranger, "s-out").await, None);
    assert!(
        !db.store
            .is_member(&stranger, &first.project_id)
            .await
            .unwrap()
    );
    assert!(
        !db.store
            .is_member(&stranger, &second.project_id)
            .await
            .unwrap(),
        "a non-member learns nothing and joins nothing"
    );

    let (local, _) = db
        .store
        .init_project(&member, "local:widget", None)
        .await
        .unwrap();
    db.store
        .capture(
            &member,
            repo_observation("local", "s-local", "local:widget"),
        )
        .await
        .unwrap();
    assert_eq!(
        session_project(&db, &member, "s-local").await,
        None,
        "local: is not a repository fallback"
    );
    assert_ne!(
        session_project(&db, &member, "s-local").await.as_deref(),
        Some(local.project_id.as_str())
    );
    db.finish().await;
}
