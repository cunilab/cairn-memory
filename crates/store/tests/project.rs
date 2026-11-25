use cairn_core::capture::WorkContext;
use cairn_store::StoreError;
use cairn_store::testing::{TestDb, rule, save_input};

#[tokio::test]
async fn init_join_rename_transfer_and_members() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let owner = db.person("owner").await;
    let member = db.person("member").await;
    let stranger = db.person("stranger").await;

    let (project, created) = store
        .init_project(&owner, "github.com/acme/cairn", None)
        .await
        .unwrap();
    assert!(created);
    assert_eq!(project.name, "cairn");
    let (again, created) = store
        .init_project(&owner, "github.com/acme/cairn", Some("cairn"))
        .await
        .unwrap();
    assert!(!created && again.project_id == project.project_id);
    assert!(
        store
            .init_project(&owner, "github.com/acme/cairn", Some("renamed"))
            .await
            .is_err()
    );

    assert_eq!(
        store
            .join_project(&member, &project.project_id, Some("old name"))
            .await
            .unwrap()
            .name,
        "cairn"
    );
    store
        .join_project(&member, &project.project_id, Some("cairn"))
        .await
        .unwrap();
    store
        .join_project(&member, &project.project_id, None)
        .await
        .unwrap();
    assert!(matches!(
        store.join_project(&member, "proj_missing", None).await,
        Err(StoreError::NotFound(_))
    ));

    let members = store
        .project_members(&member, &project.project_id)
        .await
        .unwrap();
    assert_eq!(members.len(), 2);
    assert!(members.iter().any(|m| m.is_owner && m.person_id == owner));
    assert!(
        store
            .project_members(&stranger, &project.project_id)
            .await
            .is_err()
    );

    assert!(
        store
            .rename_project(&member, &project.project_id, "x")
            .await
            .is_err(),
        "owner only"
    );
    assert_eq!(
        store
            .rename_project(&owner, &project.project_id, "Cairn 2")
            .await
            .unwrap()
            .name,
        "Cairn 2"
    );

    assert!(
        store
            .transfer_project(&owner, false, &project.project_id, &stranger)
            .await
            .is_err(),
        "target must be a member"
    );
    assert!(
        store
            .transfer_project(&member, false, &project.project_id, &member)
            .await
            .is_err(),
        "only the owner"
    );
    store
        .transfer_project(&owner, false, &project.project_id, &member)
        .await
        .unwrap();
    assert_eq!(
        store.list_projects(&owner).await.unwrap()[0].owner_id,
        member
    );
    assert!(store.list_projects(&stranger).await.unwrap().is_empty());
    db.finish().await;
}

#[tokio::test]
async fn branches_and_conflicts_are_derived_from_memories() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let owner = db.person("owner").await;
    let (project, _) = store
        .init_project(&owner, "github.com/acme/cairn", None)
        .await
        .unwrap();
    let on = |branch: &str| WorkContext {
        project_id: Some(project.project_id.clone()),
        branch: Some(branch.into()),
        ..WorkContext::default()
    };
    let a = store
        .save(&owner, rule("Use tabs"), &on("main"))
        .await
        .unwrap();
    store
        .save(&owner, rule("Use spaces"), &on("main"))
        .await
        .unwrap();
    let mut conflicting = save_input(
        serde_json::json!({"kind":"rule","title":"Use spaces now","summary":"s is a summary of the test","reason":"r and the test says why"}),
    );
    conflicting.contradicts = Some(a.id.clone());
    store
        .save(&owner, conflicting, &on("feature"))
        .await
        .unwrap();

    let branches = store
        .project_branches(&owner, &project.project_id, false)
        .await
        .unwrap();
    let main = branches.iter().find(|b| b.branch == "main").expect("main");
    assert_eq!(main.memories, 2);
    assert!(
        branches
            .iter()
            .any(|b| b.branch == "feature" && b.memories == 1)
    );

    let conflicts = store
        .project_conflicts(&owner, &project.project_id, false)
        .await
        .unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].contradicts_id, a.id);
    store.forget(&owner, &a.id).await.unwrap();
    assert!(
        store
            .project_conflicts(&owner, &project.project_id, false)
            .await
            .unwrap()
            .is_empty()
    );
    db.finish().await;
}

#[tokio::test]
async fn auto_join_audited_once() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let owner = db.person("owner").await;
    let member = db.person("member").await;
    let (project, _) = db
        .store
        .init_project(&owner, "github.com/acme/repo", None)
        .await
        .unwrap();
    for event in ["join-one", "join-two"] {
        let input = serde_json::from_value(serde_json::json!({"event_id":event,"session_id":"join", "kind":"prompt", "content":"working", "context":{"project_id":project.project_id}})).unwrap();
        db.store.capture(&member, input).await.unwrap();
    }
    assert!(
        db.store
            .is_member(&member, &project.project_id)
            .await
            .unwrap()
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE person_id=$1 AND action='project_auto_joined' AND subject_id=$2 AND metadata->>'source'='project_file'")
        .bind(&member).bind(&project.project_id).fetch_one(db.store.pool()).await.unwrap();
    assert_eq!(count, 1);
    db.finish().await;
}

#[tokio::test]
async fn remote_alone_never_joins() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let owner = db.person("owner").await;
    let other = db.person("other").await;
    let (project, _) = db
        .store
        .init_project(&owner, "github.com/acme/repo", None)
        .await
        .unwrap();
    let input = serde_json::from_value(serde_json::json!({"event_id":"remote-one","session_id":"remote", "kind":"prompt", "content":"working", "context":{"repository":"github.com/acme/repo"}})).unwrap();
    db.store.capture(&other, input).await.unwrap();
    assert!(
        !db.store
            .is_member(&other, &project.project_id)
            .await
            .unwrap()
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE person_id=$1 AND action='project_auto_joined'",
    )
    .bind(&other)
    .fetch_one(db.store.pool())
    .await
    .unwrap();
    assert_eq!(count, 0);
    db.finish().await;
}

#[tokio::test]
async fn archive_preserves_memory_rows_and_pauses_project_jobs() {
    use cairn_core::memory::Scope;
    use cairn_core::process::jobs::JobKind;
    use cairn_store::repo::RepoState;
    use serde_json::{Value, json};

    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let owner = db.person("archive-owner").await;
    let (project, _) = store
        .init_project(
            &owner,
            "github.com/acme/e2e-archive-jobs",
            Some("E2E Archive Jobs"),
        )
        .await
        .unwrap();
    let pid = &project.project_id;
    let work = WorkContext {
        project_id: Some(pid.clone()),
        branch: Some("main".into()),
        files: vec!["src/archive.rs".into()],
        ..WorkContext::default()
    };
    let first = store
        .save(&owner, rule("Keep archived project blueprints"), &work)
        .await
        .unwrap()
        .id;
    let mut next = rule("Keep archived project schematics");
    next.supersedes = Some(first.clone());
    let second = store.save(&owner, next.clone(), &work).await.unwrap().id;
    let mut personal = rule("Keep personal schematics");
    personal.scope = Some(Scope::Personal);
    personal.contradicts = Some(second.clone());
    let personal = store.save(&owner, personal, &work).await.unwrap().id;
    assert_eq!(
        store.get(&owner, &personal).await.unwrap().links[0].target,
        second
    );
    let contents = || async {
        sqlx::query_scalar::<_, Value>(
            "SELECT jsonb_agg(to_jsonb(m) ORDER BY memory_id) FROM memories m WHERE m.project_id = $1 AND m.scope = 'project'"
        ).bind(pid).fetch_one(store.pool()).await.unwrap()
    };
    let before = contents().await;
    let mut observation = json!({"event_id": "archive-observation", "session_id": "archive-session",
        "kind": "assistant", "content": "Decision: Keep paper copies of schematics\nReason: offline access",
        "context": work});
    observation["context"]["worktree"] = json!("e2e-archive");
    store
        .capture(&owner, serde_json::from_value(observation).unwrap())
        .await
        .unwrap();
    store.end_session(&owner, "archive-session").await.unwrap();
    store
        .enqueue_job(JobKind::Consolidate, &second)
        .await
        .unwrap();

    store
        .set_project_archived(&owner, false, pid, true)
        .await
        .unwrap();
    assert!(matches!(
        store.get(&owner, &first).await,
        Err(StoreError::NotFound(_))
    ));
    assert!(
        matches!(store.save(&owner, next, &work).await, Err(StoreError::Conflict(message)) if message.contains("E2E Archive Jobs is archived"))
    );
    assert!(store.get(&owner, &personal).await.unwrap().links.is_empty());
    assert!(
        store
            .brief_check(&owner, pid, "main")
            .await
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        store
            .repo_state(
                &owner,
                &RepoState {
                    project_id: pid.clone(),
                    branch: "main".into(),
                    files_missing: work.files.clone(),
                    ..RepoState::default()
                }
            )
            .await,
        Err(StoreError::Conflict(_))
    ));
    assert!(matches!(
        store.distill_session(&owner, "archive-session").await,
        Err(StoreError::Conflict(_))
    ));
    assert!(matches!(
        store.consolidate_memory(&second).await,
        Err(StoreError::Conflict(_))
    ));
    assert!(matches!(
        store.link_memory(&second).await,
        Err(StoreError::Conflict(_))
    ));
    for kind in [
        JobKind::Distill,
        JobKind::Consolidate,
        JobKind::Link,
        JobKind::Embed,
    ] {
        for job in store.claim_jobs(kind, 50).await.unwrap() {
            assert_eq!(job.key, personal);
        }
    }
    assert_eq!(contents().await, before);

    store
        .set_project_archived(&owner, false, pid, false)
        .await
        .unwrap();
    assert_eq!(contents().await, before);
    let restored = store.get(&owner, &second).await.unwrap();
    assert!(restored.links.iter().any(|link| link.target == first));
    assert_eq!(
        store.get(&owner, &personal).await.unwrap().links[0].target,
        second
    );
    for kind in [
        JobKind::Distill,
        JobKind::Consolidate,
        JobKind::Link,
        JobKind::Embed,
    ] {
        assert!(
            !store.claim_jobs(kind, 50).await.unwrap().is_empty(),
            "{kind:?}"
        );
    }
    db.finish().await;
}
