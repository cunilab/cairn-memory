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

    assert!(
        store
            .join_project(&member, &project.project_id, Some("wrong"))
            .await
            .is_err()
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
        .project_branches(&owner, &project.project_id)
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
        .project_conflicts(&owner, &project.project_id)
        .await
        .unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].contradicts_id, a.id);
    store.forget(&owner, &a.id).await.unwrap();
    assert!(
        store
            .project_conflicts(&owner, &project.project_id)
            .await
            .unwrap()
            .is_empty()
    );
    db.finish().await;
}
