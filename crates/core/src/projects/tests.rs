use crate::accounts::WEB_ONLY_ERROR;
use crate::lifecycle::{LifecycleInput, MemoryKind};
use crate::ownership::MemoryOwnerRecord;
use crate::projects::{Project, ProjectEntityInput};
use crate::store::MemoryStore;
use crate::test_support::{
    assert_unknown_id, capture_in, context, failed_write_rolls_back_then_retry_persists,
    join_project, owned_project, project_row, remember_with_context, repo_context, temp_store_dir,
    temp_store_path,
};
use crate::types::{StoreError, WorkContext};
use std::fs;

// `project_maintenance` is owner-only. Members read what is explicitly shared with them.
#[test]
fn project_maintenance_is_owner_only_and_needs_an_owned_project() -> Result<(), StoreError> {
    let path = temp_store_path("maintenance-owner");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let member = store.register_account("member@example.test", "correct horse battery staple")?;
    let outsider =
        store.register_account("outsider@example.test", "correct horse battery staple")?;
    let (project_id, mine) = owned_project(&mut store, &owner.token, "maintain owner decision")?;
    join_project(&mut store, &member.token, &project_id)?;
    store.share_project_memory(
        &owner.token,
        &project_id,
        &mine.memory_id,
        "member@example.test",
    )?;
    store.create_project_team(&owner.token, &project_id, "Core")?;
    let before = fs::read(&path)?;

    // Owner: unchanged report.
    let report = store.project_maintenance(&owner.token, &project_id, Some("other-commit"))?;
    assert_eq!(report.active_memory_ids, vec![mine.memory_id.clone()]);
    assert_eq!(report.stale_memory_ids, vec![mine.memory_id.clone()]);

    // A current member and an outsider who know the id are refused, and
    // the error carries no memory id or commit.
    for token in [&member.token, &outsider.token] {
        let error = store
            .project_maintenance(token, &project_id, Some("other-commit"))
            .expect_err("only the owner reads maintenance");
        assert!(
            matches!(&error, StoreError::Validation(text) if text == "account does not own the project")
        );
    }
    assert_eq!(fs::read(&path)?, before, "a read never writes");
    let error = store
        .project_maintenance(&owner.token, "project-missing", None)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unknown project: project-missing")
    );
    assert!(
        store
            .project_maintenance("not-a-token", &project_id, None)
            .unwrap_err()
            .to_string()
            .contains("account authentication failed")
    );

    // A project with no owner record is not maintainable by any account until the
    // existing registration claims it; there is no other claim.
    store
        .state
        .project_owners
        .retain(|record| record.project_id != project_id);
    for token in [&owner.token, &outsider.token] {
        let error = store
            .project_maintenance(token, &project_id, None)
            .expect_err("an ownerless project is denied");
        assert!(
            matches!(&error, StoreError::Validation(text) if text == "account does not own the project")
        );
    }
    store.register_project(&owner.token, "cairn-demo", "Demo", None)?;
    assert!(
        store
            .project_maintenance(&owner.token, &project_id, None)
            .is_ok()
    );
    assert!(
        store
            .project_maintenance(&outsider.token, &project_id, None)
            .is_err()
    );
    fs::remove_file(path)?;
    Ok(())
}

// The Layer 4 registry (branches, worktrees, entities, freshness, merge, close) is
// owner-only, and a merge cannot copy another account's memory.
#[test]
fn project_registry_is_owner_only_for_reads_and_writes() -> Result<(), StoreError> {
    const DENIED: &str = "account does not own the project";
    let path = temp_store_path("registry-owner");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let member = store.register_account("member@example.test", "correct horse battery staple")?;
    let outsider =
        store.register_account("outsider@example.test", "correct horse battery staple")?;
    let (project_id, shared) = owned_project(&mut store, &owner.token, "registry owner decision")?;
    join_project(&mut store, &member.token, &project_id)?;
    store.share_project_memory(
        &owner.token,
        &project_id,
        &shared.memory_id,
        "member@example.test",
    )?;
    store.create_project_team(&owner.token, &project_id, "Core")?;
    let branch = store.register_branch(&owner.token, &project_id, "feat/auth", None, None, None)?;
    let source = remember_with_context(&mut store, &owner.token, "registry merge source")?;
    let entity = || ProjectEntityInput {
        kind: "file".to_owned(),
        key: "src/secret.rs".to_owned(),
        content: None,
        source_memory_id: None,
        commit: None,
    };
    store.register_project_entity(&owner.token, &project_id, entity())?;
    let before = fs::read(&path)?;

    type Op = fn(&mut MemoryStore, &str, &str, &str, &str) -> Result<(), StoreError>;
    let ops: [(&str, Op); 9] = [
        ("register_branch", |s, t, p, _, _| {
            s.register_branch(t, p, "feat/evil", None, None, None)
                .map(drop)
        }),
        ("branches", |s, t, p, _, _| s.branches(t, p).map(drop)),
        ("register_worktree", |s, t, p, _, _| {
            s.register_worktree(t, p, None, "/wt/evil", None).map(drop)
        }),
        ("worktrees", |s, t, p, _, _| s.worktrees(t, p).map(drop)),
        ("project_entities", |s, t, p, _, _| {
            s.project_entities(t, p).map(drop)
        }),
        ("register_project_entity", |s, t, p, _, _| {
            let input = ProjectEntityInput {
                kind: "file".to_owned(),
                key: "evil.rs".to_owned(),
                content: None,
                source_memory_id: None,
                commit: None,
            };
            s.register_project_entity(t, p, input).map(drop)
        }),
        ("freshness_history", |s, t, p, _, _| {
            s.freshness_history(t, p).map(drop)
        }),
        ("close_branch", |s, t, p, b, _| {
            s.close_branch(t, p, b, "closing").map(drop)
        }),
        ("confirm_branch_merge", |s, t, p, b, m| {
            s.confirm_branch_merge(t, p, b, m, "abc123", "merge")
                .map(drop)
        }),
    ];
    // A current member and an outsider who know every id are refused on
    // every operation, with one error that carries no registry content, and nothing is written.
    for (name, op) in &ops {
        for token in [&member.token, &outsider.token] {
            let error = op(
                &mut store,
                token,
                &project_id,
                &branch.branch_id,
                &source.memory_id,
            )
            .expect_err(name);
            assert!(
                matches!(&error, StoreError::Validation(text) if text == DENIED),
                "{name}: {error}"
            );
        }
        let error = op(&mut store, "not-a-token", &project_id, "b", "m").unwrap_err();
        assert!(
            error.to_string().contains("account authentication failed"),
            "{name}"
        );
        let error = op(&mut store, &owner.token, "project-missing", "b", "m").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown project: project-missing"),
            "{name}"
        );
    }
    assert_eq!(fs::read(&path)?, before, "denials and errors write nothing");
    assert_eq!(
        MemoryStore::open(&path)?
            .branches(&owner.token, &project_id)?
            .len(),
        1
    );

    // The owner reads and writes normally.
    assert_eq!(store.branches(&owner.token, &project_id)?.len(), 1);
    store.register_worktree(
        &owner.token,
        &project_id,
        Some(branch.branch_id.clone()),
        "/wt/ok",
        None,
    )?;
    assert_eq!(store.worktrees(&owner.token, &project_id)?.len(), 1);
    assert!(
        !store
            .project_entities(&owner.token, &project_id)?
            .is_empty()
    );
    assert!(
        !store
            .freshness_history(&owner.token, &project_id)?
            .is_empty()
    );
    let merged = store.confirm_branch_merge(
        &owner.token,
        &project_id,
        &branch.branch_id,
        &source.memory_id,
        "abc123",
        "merge",
    )?;
    assert_eq!(merged.branch.status, "merged");
    let other = store.register_branch(&owner.token, &project_id, "feat/old", None, None, None)?;
    assert_eq!(
        store
            .close_branch(&owner.token, &project_id, &other.branch_id, "done")?
            .status,
        "closed"
    );

    // A merge cannot copy another account's memory into the caller's own project.
    let own = store
        .register_project(&outsider.token, "cairn-demo", "Mine", None)?
        .project_id;
    let own_branch = store.register_branch(&outsider.token, &own, "feat/auth", None, None, None)?;
    let victim = remember_with_context(&mut store, &owner.token, "registrymerge private text")?;
    let error = store
        .confirm_branch_merge(
            &outsider.token,
            &own,
            &own_branch.branch_id,
            &victim.memory_id,
            "abc123",
            "merge",
        )
        .expect_err("another account's memory is not a merge source");
    assert_unknown_id(&error, &victim.memory_id);
    assert!(store.project_memories(&outsider.token, &own)?.is_empty());
    assert_eq!(store.branches(&outsider.token, &own)?[0].status, "active");
    // A branch id from another project does not resolve in the caller's project.
    assert!(
        store
            .register_worktree(
                &outsider.token,
                &own,
                Some(branch.branch_id.clone()),
                "/wt/x",
                None
            )
            .is_err()
    );

    // A project with no owner record is not registry-accessible until registration claims it.
    store
        .state
        .project_owners
        .retain(|record| record.project_id != project_id);
    assert!(store.branches(&owner.token, &project_id).is_err());
    assert!(
        store
            .register_branch(&outsider.token, &project_id, "feat/z", None, None, None)
            .is_err()
    );
    store.register_project(&owner.token, "cairn-demo", "Demo", None)?;
    assert!(store.branches(&owner.token, &project_id).is_ok());
    assert!(store.branches(&outsider.token, &project_id).is_err());
    fs::remove_file(path)?;
    Ok(())
}

// CM-AUDIT-015 B1: a merge confirmation must not use a stored foreign row as the promotion.
#[test]
fn stored_foreign_project_rows_do_not_alias_a_branch_merge() -> Result<(), StoreError> {
    let path = temp_store_path("merge-foreign-row");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let outsider =
        store.register_account("outsider@example.test", "correct horse battery staple")?;
    let (project_id, _) = owned_project(&mut store, &owner.token, "merge first decision")?;
    let branch = store.register_branch(&owner.token, &project_id, "feat/auth", None, None, None)?;
    let source = remember_with_context(&mut store, &owner.token, "merge owner source")?;
    let mut legacy_ctx = context();
    legacy_ctx.branch = Some("feat/legacy".to_owned());
    let legacy_branch =
        store.register_branch(&owner.token, &project_id, "feat/legacy", None, None, None)?;
    let legacy_source = store.remember_for_account(
        &owner.token,
        "merge legacy source",
        Vec::new(),
        legacy_ctx,
        LifecycleInput::default(),
        MemoryKind::Explicit,
    )?;

    // Stored before the import rule: a foreign-owned row naming the owner's unmerged
    // source, and an unowned local row naming another source.
    let mut foreign = project_row("memory-foreign-merge", &project_id, "foreignmerge text");
    foreign.promoted_from = Some(source.memory_id.clone());
    let mut local = project_row("memory-local-merge", &project_id, "localmerge text");
    local.promoted_from = Some(legacy_source.memory_id.clone());
    store.state.memory_owners.push(MemoryOwnerRecord {
        memory_id: foreign.memory_id.clone(),
        person_id: outsider.account.person_id.clone(),
    });
    let foreign_before = foreign.clone();
    store.state.memories.extend([foreign, local]);
    store.persist()?;

    let merged = store.confirm_branch_merge(
        &owner.token,
        &project_id,
        &branch.branch_id,
        &source.memory_id,
        "mc1",
        "merge",
    )?;
    assert_ne!(merged.memory.memory_id, "memory-foreign-merge");
    assert_eq!(
        merged.branch.merge_memory_id.as_deref(),
        Some(merged.memory.memory_id.as_str())
    );
    assert!(store.memory_owned_by(&merged.memory.memory_id, &owner.account.person_id));
    assert_eq!(
        merged.memory.promoted_from.as_deref(),
        Some(source.memory_id.as_str())
    );
    let listed = store.project_memories(&owner.token, &project_id)?;
    assert!(
        listed
            .iter()
            .any(|m| m.memory_id == merged.memory.memory_id)
    );
    assert!(listed.iter().all(|m| m.memory_id != "memory-foreign-merge"));

    // The foreign row is untouched: content, commit, freshness and owner; its owner still exports it.
    let stored = store
        .state
        .memories
        .iter()
        .find(|m| m.memory_id == "memory-foreign-merge")
        .expect("kept");
    assert_eq!(stored, &foreign_before);
    assert!(store.memory_owned_by("memory-foreign-merge", &outsider.account.person_id));
    assert!(
        store
            .export_for_account(&outsider.token)?
            .memories
            .iter()
            .any(|m| m.memory_id == "memory-foreign-merge")
    );
    assert!(
        store
            .freshness_history(&owner.token, &project_id)?
            .iter()
            .all(|event| event.memory_id != "memory-foreign-merge")
    );
    assert!(
        store
            .project_entities(&owner.token, &project_id)?
            .iter()
            .all(|entity| !entity.key.contains("memory-foreign-merge")
                && entity.source_memory_id.as_deref() != Some("memory-foreign-merge"))
    );
    // The owner's repeated merge is idempotent.
    let again = store.confirm_branch_merge(
        &owner.token,
        &project_id,
        &branch.branch_id,
        &source.memory_id,
        "mc1",
        "merge",
    )?;
    assert_eq!(again.memory.memory_id, merged.memory.memory_id);

    // Control: an unowned local row naming the source is still the idempotent promotion.
    let local_merge = store.confirm_branch_merge(
        &owner.token,
        &project_id,
        &legacy_branch.branch_id,
        &legacy_source.memory_id,
        "mc2",
        "merge",
    )?;
    assert_eq!(local_merge.memory.memory_id, "memory-local-merge");
    let local_again = store.confirm_branch_merge(
        &owner.token,
        &project_id,
        &legacy_branch.branch_id,
        &legacy_source.memory_id,
        "mc2",
        "merge",
    )?;
    assert_eq!(local_again.memory.memory_id, "memory-local-merge");
    fs::remove_file(path)?;
    Ok(())
}

// CM-AUDIT-015 R1: a branch merged to a foreign row by the old behaviour is refused, not read.
#[test]
fn merged_branch_aliased_to_a_foreign_row_is_not_returned() -> Result<(), StoreError> {
    let path = temp_store_path("merge-aliased-row");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let outsider =
        store.register_account("outsider@example.test", "correct horse battery staple")?;
    let (project_id, _) = owned_project(&mut store, &owner.token, "alias first decision")?;
    let branch = store.register_branch(&owner.token, &project_id, "feat/auth", None, None, None)?;
    let source = remember_with_context(&mut store, &owner.token, "alias owner source")?;
    let mut foreign = project_row("memory-foreign-alias", &project_id, "foreignalias text");
    foreign.promoted_from = Some(source.memory_id.clone());
    // The same shape in another project of the same owner: also not this project's row.
    let other_project = store
        .register_project(&owner.token, "other-repo", "Other", None)?
        .project_id;
    let mut elsewhere = project_row(
        "memory-elsewhere-alias",
        &other_project,
        "elsewherealias text",
    );
    elsewhere.promoted_from = Some(source.memory_id.clone());
    store.state.memory_owners.push(MemoryOwnerRecord {
        memory_id: foreign.memory_id.clone(),
        person_id: outsider.account.person_id.clone(),
    });
    store.state.memories.extend([foreign, elsewhere]);
    // Old state: the branch was marked merged and aliased to the foreign row.
    for target in ["memory-foreign-alias", "memory-elsewhere-alias"] {
        let stored = store
            .state
            .project_branches
            .iter_mut()
            .find(|b| b.branch_id == branch.branch_id)
            .expect("branch");
        stored.status = "merged".to_owned();
        stored.merge_commit = Some("mc1".to_owned());
        stored.merge_memory_id = Some(target.to_owned());
        store.persist()?;
        let before = fs::read(&path)?;
        let error = store
            .confirm_branch_merge(
                &owner.token,
                &project_id,
                &branch.branch_id,
                &source.memory_id,
                "mc1",
                "merge",
            )
            .expect_err("the aliased row is not returned");
        assert!(
            matches!(&error, StoreError::Validation(text) if text == "project branch is already merged")
        );
        assert!(!error.to_string().contains("alias text"));
        assert_eq!(fs::read(&path)?, before, "nothing is written or repaired");
        assert_eq!(
            store.branches(&owner.token, &project_id)?[0]
                .merge_memory_id
                .as_deref(),
            Some(target)
        );
    }
    assert!(
        store
            .export_for_account(&outsider.token)?
            .memories
            .iter()
            .any(|m| m.memory_id == "memory-foreign-alias")
    );
    // Controls: the outsider is still refused, and a bad token fails on authentication.
    assert!(
        store
            .confirm_branch_merge(
                &outsider.token,
                &project_id,
                &branch.branch_id,
                &source.memory_id,
                "mc1",
                "merge"
            )
            .is_err()
    );
    assert!(
        store
            .confirm_branch_merge(
                "not-a-token",
                &project_id,
                &branch.branch_id,
                &source.memory_id,
                "mc1",
                "merge"
            )
            .unwrap_err()
            .to_string()
            .contains("account authentication failed")
    );
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn failed_project_and_team_writes_roll_back_and_retry_persists() -> Result<(), StoreError> {
    let dir = temp_store_dir("project-rollback");
    let path = dir.join("store.json");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let member_id = store
        .register_account("member@example.test", "correct horse battery staple")?
        .account
        .person_id;
    let token = owner.token;
    let member = "member@example.test";

    failed_write_rolls_back_then_retry_persists(&mut store, &path, |s| {
        s.register_project(&token, "cairn-demo", "Demo", None)
            .map(|_| ())
    })?;
    let project_id = store.state.projects[0].project_id.clone();
    let source = remember_with_context(&mut store, &token, "rollback source")?;
    let promoted = store.promote_memory(&token, &project_id, &source.memory_id, "why")?;

    let joining = WorkContext {
        project_id: Some(project_id.clone()),
        ..WorkContext::default()
    };
    failed_write_rolls_back_then_retry_persists(&mut store, &path, |s| {
        s.join_context_project(&member_id, &joining).map(|_| ())
    })?;
    failed_write_rolls_back_then_retry_persists(&mut store, &path, |s| {
        s.share_project_memory(&token, &project_id, &promoted.memory_id, member)
            .map(|_| ())
    })?;
    failed_write_rolls_back_then_retry_persists(&mut store, &path, |s| {
        s.revoke_project_share(&token, &project_id, &promoted.memory_id, member)
            .map(|_| ())
    })?;
    failed_write_rolls_back_then_retry_persists(&mut store, &path, |s| {
        s.create_project_team(&token, &project_id, "Core")
            .map(|_| ())
    })?;
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn a_project_with_a_member_needs_the_web_session_to_merge() -> Result<(), StoreError> {
    let path = temp_store_path("device-merge-gate");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let member = store.register_account("member@example.test", "correct horse battery staple")?;
    let device = store.register_device("device-own", &owner.account.person_id, None, "Laptop")?;
    let (project_id, _) = owned_project(&mut store, &device.token, "device remove decision")?;
    let branch =
        store.register_branch(&device.token, &project_id, "feat/auth", None, None, None)?;
    let source = remember_with_context(&mut store, &device.token, "gate merge source")?;
    join_project(&mut store, &member.token, &project_id)?;

    let blocked = store
        .confirm_branch_merge(
            &device.token,
            &project_id,
            &branch.branch_id,
            &source.memory_id,
            "c1",
            "gate",
        )
        .expect_err("a project with members still needs the web console");
    assert!(blocked.to_string().contains(WEB_ONLY_ERROR));
    store.confirm_branch_merge(
        &owner.token,
        &project_id,
        &branch.branch_id,
        &source.memory_id,
        "c1",
        "gate",
    )?;
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn repromoting_a_forgotten_copy_creates_a_live_copy() -> Result<(), StoreError> {
    let path = temp_store_path("repromote-forgotten");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let (project_id, first) = owned_project(&mut store, &owner.token, "repromote personal fact")?;
    let source_id = first.promoted_from.clone().expect("origin");
    store.forget_for_account(&owner.token, &first.memory_id)?;
    let second = store.promote_memory(&owner.token, &project_id, &source_id, "again")?;
    assert_ne!(second.memory_id, first.memory_id);
    assert!(!store.state.forgotten_ids.contains(&second.memory_id));
    let listed = store.project_memories(&owner.token, &project_id)?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].memory_id, second.memory_id);
    // A live copy is still returned unchanged on a repeat.
    let repeat = store.promote_memory(&owner.token, &project_id, &source_id, "again")?;
    assert_eq!(repeat.memory_id, second.memory_id);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn capture_registers_project_and_branch_once() -> Result<(), StoreError> {
    let path = temp_store_path("auto-register");
    let mut store = MemoryStore::open(&path)?;
    let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
    capture_in(
        &mut store,
        &a.token,
        "e1",
        repo_context("  auto-repo ", Some("feat/x")),
    )?;
    capture_in(
        &mut store,
        &a.token,
        "e2",
        repo_context("auto-repo", Some("feat/x")),
    )?;
    capture_in(
        &mut store,
        &a.token,
        "e3",
        repo_context("auto-repo", Some("HEAD")),
    )?;
    capture_in(&mut store, &a.token, "e4", repo_context("auto-repo", None))?;
    capture_in(
        &mut store,
        &a.token,
        "e5",
        repo_context("auto-repo", Some("  ")),
    )?;
    capture_in(&mut store, &a.token, "e6", WorkContext::default())?;
    let projects = store.projects(&a.token)?;
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].repository, "auto-repo");
    let project_id = projects[0].project_id.clone();
    assert_eq!(
        store.project_owner_id(&project_id).as_deref(),
        Some(a.account.person_id.as_str())
    );
    let branches = store.branches(&a.token, &project_id)?;
    assert_eq!(branches.len(), 1);
    assert_eq!(branches[0].name, "feat/x");
    assert_eq!(branches[0].status, "active");
    // The explicit registration returns the same project.
    let explicit = store.register_project(&a.token, "auto-repo", "Auto", None)?;
    assert_eq!(explicit.project_id, project_id);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn capture_uses_a_shared_project_and_never_claims_an_ownerless_one() -> Result<(), StoreError> {
    let path = temp_store_path("auto-register-member");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
    let member = store.register_account("m@example.test", "correct horse battery staple")?;
    let outsider = store.register_account("x@example.test", "correct horse battery staple")?;
    let shared = store.register_project(&owner.token, "shared-repo", "Shared", None)?;
    join_project(&mut store, &member.token, &shared.project_id)?;

    capture_in(
        &mut store,
        &member.token,
        "m1",
        repo_context("shared-repo", Some("feat/m")),
    )?;
    assert_eq!(
        store.projects(&member.token)?.len(),
        1,
        "no second project for the member"
    );
    assert_eq!(store.state.projects.len(), 1);
    assert!(
        store.state.project_branches.is_empty(),
        "branch writes stay owner-only"
    );

    capture_in(
        &mut store,
        &owner.token,
        "o1",
        repo_context("shared-repo", Some("feat/o")),
    )?;
    assert_eq!(store.branches(&owner.token, &shared.project_id)?.len(), 1);

    // A project with no owner record stays ownerless; the capturer gets their own.
    let ownerless = Project {
        project_id: "project-ownerless".to_owned(),
        repository: "legacy-repo".to_owned(),
        name: "Legacy".to_owned(),
        default_branch: None,
        created_at: "1".to_owned(),
        updated_at: "1".to_owned(),
    };
    store.state.projects.push(ownerless);
    capture_in(
        &mut store,
        &outsider.token,
        "x1",
        repo_context("legacy-repo", Some("main")),
    )?;
    assert_eq!(store.project_owner_id("project-ownerless"), None);
    let mine = store.projects(&outsider.token)?;
    assert!(mine.iter().any(|p| p.repository == "legacy-repo"
        && p.project_id != "project-ownerless"
        && store.project_owner_id(&p.project_id).as_deref()
            == Some(outsider.account.person_id.as_str())));
    assert!(
        store
            .state
            .project_branches
            .iter()
            .all(|b| b.project_id != "project-ownerless")
    );
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn auto_registration_validates_the_repository_length() -> Result<(), StoreError> {
    let path = temp_store_path("auto-register-length");
    let mut store = MemoryStore::open(&path)?;
    let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
    capture_in(
        &mut store,
        &a.token,
        "long-1",
        repo_context(&"r".repeat(513), Some("main")),
    )?;
    assert!(store.projects(&a.token)?.is_empty());
    assert!(store.state.project_branches.is_empty());
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn init_project_creates_once_defaults_the_name_and_is_per_owner() -> Result<(), StoreError> {
    let path = temp_store_path("init-project");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
    let other = store.register_account("x@example.test", "correct horse battery staple")?;
    let (created, is_new) = store.init_project(&owner.token, " repo-a ", None)?;
    assert!(is_new);
    assert_eq!(
        (created.repository.as_str(), created.name.as_str()),
        ("repo-a", "repo-a")
    );
    let (again, is_new) = store.init_project(&owner.token, "repo-a", Some("Renamed"))?;
    assert!(!is_new);
    assert_eq!(again.project_id, created.project_id);
    assert_eq!(again.name, "repo-a");
    let (theirs, is_new) = store.init_project(&other.token, "repo-a", Some("Named"))?;
    assert!(is_new);
    assert_ne!(theirs.project_id, created.project_id);
    assert_eq!(theirs.name, "Named");
    assert!(store.init_project(&owner.token, "  ", None).is_err());
    assert!(
        store
            .init_project(&owner.token, &"r".repeat(513), None)
            .is_err()
    );
    fs::remove_file(path)?;
    Ok(())
}
