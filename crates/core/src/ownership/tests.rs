use crate::projects::Project;
use crate::store::MemoryStore;
use crate::test_support::{
    capture_in, context, import_forgotten, join_project, observation_for, owned_project,
    project_row, remember_with_context, repo_context, temp_store_path, with_project_id,
};
use crate::types::{StoreError, WorkContext};
use std::fs;

#[test]
fn account_import_always_keeps_rows_personal_even_with_forged_publication() -> Result<(), StoreError>
{
    let path = temp_store_path("personal-import");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
    let other = store.register_account("m@example.test", "correct horse battery staple")?;
    let (project_id, _) = owned_project(&mut store, &owner.token, "published fact")?;
    join_project(&mut store, &other.token, &project_id)?;
    let mut row = project_row("forged-decision", &project_id, "import stays private");
    row.published_by = Some(owner.account.person_id.clone());
    import_forgotten(&mut store, &other.token, vec![row], &[])?;
    assert_eq!(store.project_memories(&other.token, &project_id)?.len(), 1);
    let exported = store.export_for_account(&other.token)?;
    assert_eq!(exported.memories.len(), 1);
    assert_eq!(exported.memories[0].published_by, None);
    assert_eq!(exported.memories[0].project_id, None);
    std::fs::remove_file(path)?;
    Ok(())
}

#[test]
fn a_capture_with_a_project_id_makes_the_capturer_a_member_once() -> Result<(), StoreError> {
    let path = temp_store_path("join-by-project-file");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
    let member = store.register_account("m@example.test", "correct horse battery staple")?;
    let shared = store.register_project(&owner.token, "cairn-demo", "Shared", None)?;

    // The member's repository name differs, so only the project id can link it.
    let context = with_project_id(
        repo_context("other-name", Some("feat/m")),
        &shared.project_id,
    );
    capture_in(&mut store, &member.token, "j1", context.clone())?;
    capture_in(&mut store, &member.token, "j2", context)?;

    assert_eq!(
        store.state.projects.len(),
        1,
        "no duplicate private project"
    );
    assert_eq!(store.state.project_branches.len(), 1);
    let projects = store.projects(&member.token)?;
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].project_id, shared.project_id);
    let team = store.project_people(&owner.token, &shared.project_id)?;
    assert_eq!(team.len(), 2, "joining twice records one membership");
    assert_eq!(team[1].email, "m@example.test");
    let stored = store
        .observations()
        .find(|observation| observation.event_id == "j1")
        .expect("the observation is stored");
    assert_eq!(
        stored.context.project_id.as_deref(),
        Some(shared.project_id.as_str())
    );
    let audit = store.audit_events(&owner.token, 10)?;
    assert_eq!(
        audit
            .iter()
            .filter(|event| event.action == "project_member_joined")
            .count(),
        1
    );

    // The new member can be shared with, and reads the shared decision.
    let source = remember_with_context(&mut store, &owner.token, "member shared source")?;
    store.promote_memory(&owner.token, &shared.project_id, &source.memory_id, "why")?;
    assert_eq!(
        store
            .project_memories(&member.token, &shared.project_id)?
            .len(),
        1
    );
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn unknown_and_ownerless_ids_stay_personal_while_own_project_attaches() -> Result<(), StoreError> {
    let path = temp_store_path("join-ignored");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
    let other = store.register_account("x@example.test", "correct horse battery staple")?;
    let own = store.register_project(&owner.token, "own-repo", "Own", None)?;

    // Unknown id: the capture succeeds and registers the repository as before.
    capture_in(
        &mut store,
        &other.token,
        "u1",
        with_project_id(repo_context("fallback-repo", Some("main")), "project-nope"),
    )?;
    let mine = store.projects(&other.token)?;
    assert!(mine.is_empty());

    // A project with no owner record is never joined.
    store.state.projects.push(Project {
        project_id: "project-ownerless".to_owned(),
        repository: "legacy-repo".to_owned(),
        name: "Legacy".to_owned(),
        default_branch: None,
        created_at: "1".to_owned(),
        updated_at: "1".to_owned(),
    });
    capture_in(
        &mut store,
        &other.token,
        "u2",
        with_project_id(repo_context("legacy-repo", None), "project-ownerless"),
    )?;
    assert!(store.state.project_members.is_empty());

    // The owner's own project id adds no membership and still registers the branch.
    capture_in(
        &mut store,
        &owner.token,
        "u3",
        with_project_id(repo_context("own-repo", Some("feat/o")), &own.project_id),
    )?;
    assert!(store.state.project_members.is_empty());
    assert_eq!(store.branches(&owner.token, &own.project_id)?.len(), 1);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn a_device_sync_push_with_a_project_id_joins_and_a_bare_id_keeps_session_context()
-> Result<(), StoreError> {
    let path = temp_store_path("join-by-push");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
    let member = store.register_account("m@example.test", "correct horse battery staple")?;
    let device = store.register_device("dev-m", &member.account.person_id, None, "Hook")?;
    let shared = store.register_project(&owner.token, "shared-repo", "Shared", None)?;

    store.start_session_for_account(&member.token, "bare", context())?;
    let mut input = observation_for("bare", "bare-1", "barefact");
    input.context = WorkContext {
        project_id: Some(shared.project_id.clone()),
        ..WorkContext::default()
    };
    let pushed = store.sync_push(&device.token, input)?;
    assert_eq!(store.projects(&member.token)?.len(), 1);
    assert_eq!(store.state.projects.len(), 1);
    assert_eq!(
        pushed.observation.context.project_id.as_deref(),
        Some(shared.project_id.as_str())
    );
    assert_eq!(
        pushed.observation.context.repository.as_deref(),
        Some("cairn-demo")
    );
    fs::remove_file(path)?;
    Ok(())
}
