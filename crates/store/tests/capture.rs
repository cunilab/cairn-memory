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
            observation(
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
    assert!(handoff.expires_at.is_some());
    assert_eq!(
        handoff.fields.to_json()["next_steps"][0],
        "TODO: wire the worker"
    );

    // Distilling again upserts the handoff and dedupes the note.
    let again = db.store.distill_session(&me, "s1").await.unwrap();
    assert!(again.handoff);
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
                serde_json::json!({"kind":"note","title":"old","summary":"s"}),
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
        (1, 1, 1)
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
