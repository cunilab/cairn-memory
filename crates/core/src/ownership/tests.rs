use crate::ownership::MemoryOwnerRecord;
use crate::projects::Project;
use crate::recall::RecallFilter;
use crate::store::MemoryStore;
use crate::test_support::{
    capture_in, context, ids_of, import_forgotten, join_project, linked_memory, observation_for,
    owned_project, project_row, remember_with_context, repo_context, temp_store_path,
    with_project_id,
};
use crate::types::{StoreError, WorkContext};
use std::fs;

#[test]
fn account_import_rejects_project_rows_the_caller_does_not_own() -> Result<(), StoreError> {
    let path = temp_store_path("project-import");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let outsider =
        store.register_account("outsider@example.test", "correct horse battery staple")?;
    let member = store.register_account("member@example.test", "correct horse battery staple")?;
    let (project_id, _) = owned_project(&mut store, &owner.token, "pimport owner decision")?;
    join_project(&mut store, &member.token, &project_id)?;
    let before = fs::read(&path)?;
    let owners_before = store.state.memory_owners.len();

    // Outsider, invited member and a missing project each reject the whole bundle:
    // the valid row and its tombstone are not stored either.
    for (token, project) in [
        (&outsider.token, project_id.as_str()),
        (&member.token, project_id.as_str()),
        (&outsider.token, "project-missing"),
    ] {
        let mut fine = linked_memory("pi-fine", "memory-unrelated");
        fine.supersedes = None;
        let bad = project_row("memory-pi-bad", project, "pibad claim");
        let error = import_forgotten(&mut store, token, vec![fine, bad], &["memory-link-pi-fine"])
            .expect_err("a foreign or missing project rejects the import");
        assert!(
            matches!(&error, StoreError::Validation(text) if text == "account does not own the project")
        );
        assert_eq!(fs::read(&path)?, before, "nothing is written on rejection");
    }
    assert_eq!(store.state.memory_owners.len(), owners_before);
    assert!(store.state.forgotten_ids.is_empty());
    let reopened = MemoryStore::open(&path)?;
    assert!(
        reopened
            .export_for_account(&outsider.token)?
            .memories
            .is_empty()
    );
    assert!(
        reopened
            .export_for_account(&member.token)?
            .memories
            .is_empty()
    );

    // Authentication comes before the project lookup.
    let mut bundle = store.export_for_account(&outsider.token)?;
    bundle.memories = vec![project_row("memory-pi-auth", &project_id, "piauth claim")];
    let error = store
        .import_for_account("not-a-token", bundle)
        .expect_err("bad token");
    assert!(error.to_string().contains("account authentication failed"));

    // A project with no owner record is not the caller's until it is claimed.
    store
        .state
        .project_owners
        .retain(|record| record.project_id != project_id);
    let legacy = project_row("memory-pi-legacy", &project_id, "pilegacy claim");
    assert!(import_forgotten(&mut store, &owner.token, vec![legacy.clone()], &[]).is_err());
    store.register_project(&owner.token, "cairn-demo", "Demo", None)?;

    // Own project: normal import.
    let result = import_forgotten(&mut store, &owner.token, vec![legacy], &[])?;
    assert_eq!(result.imported_memories, 1);
    let listed = store.project_memories(&owner.token, &project_id)?;
    assert!(listed.iter().any(|m| m.memory_id == "memory-pi-legacy"));
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn stored_foreign_project_rows_stay_out_of_owner_project_views() -> Result<(), StoreError> {
    let path = temp_store_path("project-foreign-rows");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let outsider =
        store.register_account("outsider@example.test", "correct horse battery staple")?;
    let member = store.register_account("member@example.test", "correct horse battery staple")?;
    let (project_id, mine) = owned_project(
        &mut store,
        &owner.token,
        "payments retry queue uses redis storage",
    )?;
    join_project(&mut store, &member.token, &project_id)?;
    store.share_project_memory(
        &owner.token,
        &project_id,
        &mine.memory_id,
        "member@example.test",
    )?;

    // Rows stored before the import rule: foreign-owned, and one local/unowned row.
    let note = project_row("memory-foreign-note", &project_id, "foreignnote claim");
    let mut hide = project_row("memory-foreign-hide", &project_id, "foreign replacement");
    hide.supersedes = Some(mine.memory_id.clone());
    let clash = project_row(
        "memory-foreign-clash",
        &project_id,
        "payments retry queue never uses redis storage",
    );
    let local = project_row("memory-local-note", &project_id, "localnote claim");
    for row in [&note, &hide, &clash] {
        store.state.memory_owners.push(MemoryOwnerRecord {
            memory_id: row.memory_id.clone(),
            person_id: outsider.account.person_id.clone(),
        });
    }
    store.state.memories.extend([note, hide, clash, local]);
    store.persist()?;

    let expected = {
        let mut ids = vec![mine.memory_id.clone(), "memory-local-note".to_owned()];
        ids.sort();
        ids
    };
    let check = |store: &MemoryStore| -> Result<(), StoreError> {
        let mut listed = store
            .project_memories(&owner.token, &project_id)?
            .into_iter()
            .map(|m| m.memory_id)
            .collect::<Vec<_>>();
        listed.sort();
        assert_eq!(listed, expected, "owner list: own and local rows only");
        let candidates = store
            .share_candidates(&owner.token, &project_id, "member@example.test")?
            .into_iter()
            .map(|c| c.memory_id)
            .collect::<Vec<_>>();
        assert_eq!(candidates, vec!["memory-local-note".to_owned()]);
        assert!(
            store
                .project_conflicts(&owner.token, &project_id)?
                .is_empty()
        );
        let report = store.project_maintenance(&owner.token, &project_id, None)?;
        assert!(report.superseded_memory_ids.is_empty());
        let mut active = report.active_memory_ids;
        active.sort();
        assert_eq!(active, expected);
        // Controls: recall and the team view do not change.
        let hits =
            store.search_for_account(&owner.token, "payments", 10, &RecallFilter::default())?;
        assert!(ids_of(&hits).contains(&mine.memory_id.as_str()));
        assert!(
            store
                .search_for_account(&owner.token, "foreignnote", 10, &RecallFilter::default())?
                .is_empty()
        );
        let shared = store.shared_project_memories(&member.token, &project_id)?;
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].memory.memory_id, mine.memory_id);
        Ok(())
    };
    check(&store)?;
    check(&MemoryStore::open(&path)?)?;

    // Nothing was deleted or rewritten: the outsider still exports its three rows.
    assert_eq!(store.export_for_account(&outsider.token)?.memories.len(), 3);
    fs::remove_file(path)?;
    Ok(())
}

// CM-AUDIT-014 B1/S1: promotion and direct share must not use a stored foreign row.
#[test]
fn stored_foreign_project_rows_do_not_block_promotion_or_reach_a_share() -> Result<(), StoreError> {
    let path = temp_store_path("project-foreign-promote");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let outsider =
        store.register_account("outsider@example.test", "correct horse battery staple")?;
    let member = store.register_account("member@example.test", "correct horse battery staple")?;
    let (project_id, _) = owned_project(&mut store, &owner.token, "promo first decision")?;
    join_project(&mut store, &member.token, &project_id)?;
    let source = remember_with_context(&mut store, &owner.token, "promo second source")?;
    let legacy_source = remember_with_context(&mut store, &owner.token, "promo legacy source")?;

    // Stored before the import rule: foreign rows, one naming the owner's unpromoted
    // source, and an unowned local row naming another source.
    let mut foreign = project_row("memory-foreign-promo", &project_id, "foreignpromo text");
    foreign.promoted_from = Some(source.memory_id.clone());
    let plain = project_row("memory-foreign-plain", &project_id, "foreignplain text");
    let mut local = project_row("memory-local-promo", &project_id, "localpromo text");
    local.promoted_from = Some(legacy_source.memory_id.clone());
    for row in [&foreign, &plain] {
        store.state.memory_owners.push(MemoryOwnerRecord {
            memory_id: row.memory_id.clone(),
            person_id: outsider.account.person_id.clone(),
        });
    }
    store.state.memories.extend([foreign, plain, local]);
    store.persist()?;
    let shares_before = store.state.project_shares.len();

    // The real promotion is created for the owner, not the foreign row, and repeats.
    let promoted = store.confirm_memory(
        &owner.token,
        &project_id,
        &source.memory_id,
        Some("abc123"),
        "why",
    )?;
    assert_ne!(promoted.memory_id, "memory-foreign-promo");
    assert_eq!(
        promoted.promoted_from.as_deref(),
        Some(source.memory_id.as_str())
    );
    assert!(store.memory_owned_by(&promoted.memory_id, &owner.account.person_id));
    let again = store.promote_memory(&owner.token, &project_id, &source.memory_id, "why")?;
    assert_eq!(again.memory_id, promoted.memory_id);
    let listed = store.project_memories(&owner.token, &project_id)?;
    assert!(listed.iter().any(|m| m.memory_id == promoted.memory_id));
    assert!(
        listed
            .iter()
            .all(|m| !m.memory_id.starts_with("memory-foreign"))
    );

    // An unowned local row is the owner's idempotent promotion, as before.
    let legacy =
        store.promote_memory(&owner.token, &project_id, &legacy_source.memory_id, "why")?;
    assert_eq!(legacy.memory_id, "memory-local-promo");

    // A direct share of a foreign id is rejected; verify already was. Own share works.
    for id in ["memory-foreign-plain", "memory-foreign-promo"] {
        let error = store
            .share_project_memory(&owner.token, &project_id, id, "member@example.test")
            .expect_err("a foreign row is not shareable");
        assert!(
            matches!(&error, StoreError::Validation(text) if text == &format!("unknown memory or event: {id}"))
        );
        assert!(
            store
                .verify_project_memory(&owner.token, &project_id, id, "abc123")
                .is_err()
        );
    }
    assert_eq!(store.state.project_shares.len(), shares_before);
    store.share_project_memory(
        &owner.token,
        &project_id,
        &promoted.memory_id,
        "member@example.test",
    )?;
    let shared = store.shared_project_memories(&member.token, &project_id)?;
    assert_eq!(shared.len(), 1);
    assert_eq!(shared[0].memory.memory_id, promoted.memory_id);

    // Nothing was deleted: the outsider still exports both rows.
    assert_eq!(store.export_for_account(&outsider.token)?.memories.len(), 2);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn a_capture_with_a_project_id_makes_the_capturer_a_member_once() -> Result<(), StoreError> {
    let path = temp_store_path("join-by-project-file");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
    let member = store.register_account("m@example.test", "correct horse battery staple")?;
    let shared = store.register_project(&owner.token, "cairn-demo", "Shared", None)?;
    store.create_project_team(&owner.token, &shared.project_id, "Core")?;

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
    assert!(store.state.project_branches.is_empty());
    let projects = store.projects(&member.token)?;
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].project_id, shared.project_id);
    let team = store.project_team(&owner.token, &shared.project_id)?;
    assert_eq!(
        team.members.len(),
        1,
        "joining twice records one membership"
    );
    assert_eq!(team.members[0].email, "m@example.test");
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
    let promoted =
        store.promote_memory(&owner.token, &shared.project_id, &source.memory_id, "why")?;
    store.share_project_memory(
        &owner.token,
        &shared.project_id,
        &promoted.memory_id,
        "m@example.test",
    )?;
    assert_eq!(
        store
            .shared_project_memories(&member.token, &shared.project_id)?
            .len(),
        1
    );
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn an_unknown_ownerless_or_own_project_id_is_ignored_by_a_capture() -> Result<(), StoreError> {
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
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].repository, "fallback-repo");

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
