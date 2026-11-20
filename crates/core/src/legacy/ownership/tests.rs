use crate::legacy::projects::Project;
use crate::legacy::store::MemoryStore;
use crate::legacy::test_support::{
    capture_in, context, import_forgotten, join_project, observation_for, owned_project,
    project_row, repo_context, temp_store_path, with_project_id,
};
use crate::legacy::types::{StoreError, WorkContext};
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
