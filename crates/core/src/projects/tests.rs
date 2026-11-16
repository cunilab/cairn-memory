use crate::lifecycle::{LifecycleInput, MemoryKind};
use crate::ownership::MemoryOwnerRecord;
use crate::projects::Project;
use crate::store::MemoryStore;
use crate::test_support::{
    capture_in, context, failed_write_rolls_back_then_retry_persists, join_project, owned_project,
    project_row, remember_with_context, repo_context, temp_store_dir, temp_store_path,
};
use crate::types::{StoreError, WorkContext};
use std::fs;

// Every member reads live decisions and maintenance; branches/worktrees belong to each person.
// A merge may publish only the caller’s own source.

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
    assert_ne!(local_merge.memory.memory_id, "memory-local-merge");
    let local_again = store.confirm_branch_merge(
        &owner.token,
        &project_id,
        &legacy_branch.branch_id,
        &legacy_source.memory_id,
        "mc2",
        "merge",
    )?;
    assert_eq!(local_again.memory.memory_id, local_merge.memory.memory_id);
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
fn failed_project_and_membership_writes_roll_back_and_retry_persists() -> Result<(), StoreError> {
    let dir = temp_store_dir("project-rollback");
    let path = dir.join("store.json");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let member_id = store
        .register_account("member@example.test", "correct horse battery staple")?
        .account
        .person_id;
    let token = owner.token;

    failed_write_rolls_back_then_retry_persists(&mut store, &path, |s| {
        s.register_project(&token, "cairn-demo", "Demo", None)
            .map(|_| ())
    })?;
    let project_id = store.state.projects[0].project_id.clone();
    let source = remember_with_context(&mut store, &token, "rollback source")?;
    store.promote_memory(&token, &project_id, &source.memory_id, "why")?;

    let joining = WorkContext {
        project_id: Some(project_id.clone()),
        ..WorkContext::default()
    };
    failed_write_rolls_back_then_retry_persists(&mut store, &path, |s| {
        s.join_context_project(&member_id, &joining).map(|_| ())
    })?;
    let _ = fs::remove_dir_all(&dir);
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
fn captures_attach_only_initialized_projects_and_register_personal_branches_once()
-> Result<(), StoreError> {
    let path = temp_store_path("initialized-branch");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
    let project = store.register_project(&owner.token, "repo", "Repo", None)?;
    let context = WorkContext {
        project_id: Some(project.project_id.clone()),
        repository: Some("repo".to_owned()),
        branch: Some("feat/x".to_owned()),
        ..WorkContext::default()
    };
    capture_in(&mut store, &owner.token, "one", context.clone())?;
    capture_in(&mut store, &owner.token, "two", context)?;
    assert_eq!(store.projects(&owner.token)?.len(), 1);
    assert_eq!(store.branches(&owner.token, &project.project_id)?.len(), 1);
    std::fs::remove_file(path)?;
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
        WorkContext {
            project_id: Some(shared.project_id.clone()),
            ..repo_context("shared-repo", Some("feat/m"))
        },
    )?;
    assert_eq!(
        store.projects(&member.token)?.len(),
        1,
        "no second project for the member"
    );
    assert_eq!(store.state.projects.len(), 1);
    assert_eq!(store.branches(&member.token, &shared.project_id)?.len(), 1);
    assert!(store.branches(&owner.token, &shared.project_id)?.is_empty());

    capture_in(
        &mut store,
        &owner.token,
        "o1",
        WorkContext {
            project_id: Some(shared.project_id.clone()),
            ..repo_context("shared-repo", Some("feat/o"))
        },
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
    assert!(mine.is_empty());
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
    assert!(
        store
            .init_project(&owner.token, "repo-a", Some("Renamed"))
            .is_err()
    );
    let (again, is_new) = store.init_project(&owner.token, "repo-a", Some("repo-a"))?;
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

use crate::*;

#[test]
fn all_members_read_and_promote_own_decisions_but_only_author_and_owner_edit()
-> Result<(), StoreError> {
    let path = temp_store_path("shared-decisions");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let author = store.register_account("author@example.test", "correct horse battery staple")?;
    let reader = store.register_account("reader@example.test", "correct horse battery staple")?;
    let project = store.register_project(&owner.token, "cairn-demo", "Demo", None)?;
    join_project(&mut store, &author.token, &project.project_id)?;
    join_project(&mut store, &reader.token, &project.project_id)?;
    let source = store.remember_for_account(
        &author.token,
        "modelneedle use WAL because durability",
        vec![],
        context(),
        LifecycleInput::default(),
        MemoryKind::Explicit,
    )?;
    let decision = store.promote_memory(
        &author.token,
        &project.project_id,
        &source.memory_id,
        "publish",
    )?;
    for token in [&owner.token, &author.token, &reader.token] {
        assert_eq!(store.project_memories(token, &project.project_id)?.len(), 1);
    }
    assert!(
        store
            .verify_project_memory(
                &reader.token,
                &project.project_id,
                &decision.memory_id,
                "abc"
            )
            .is_err()
    );
    assert!(
        store
            .correct_for_account(
                &reader.token,
                &decision.memory_id,
                "wrong",
                vec![],
                WorkContext::default(),
                LifecycleInput::default()
            )
            .is_err()
    );
    assert!(
        store
            .delete_for_account(&reader.token, &decision.memory_id)
            .is_err()
    );
    store.verify_project_memory(
        &owner.token,
        &project.project_id,
        &decision.memory_id,
        "abc",
    )?;
    store.correct_for_account(
        &author.token,
        &source.memory_id,
        "private correction",
        vec![],
        WorkContext::default(),
        LifecycleInput::default(),
    )?;
    assert_eq!(
        store
            .project_memories(&reader.token, &project.project_id)?
            .len(),
        1
    );
    let mut ctx = context();
    ctx.project_id = Some(project.project_id.clone());
    let filter = RecallFilter {
        session_id: None,
        context: Some(ctx),
    };
    let results = store.search_for_account(&reader.token, "modelneedle", 20, &filter)?;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].memory_id, decision.memory_id);
    assert_eq!(serde_json::to_value(&results[0])?["source"], "project");
    let corrected = store.correct_for_account(
        &owner.token,
        &decision.memory_id,
        "modelneedle use WAL safely",
        vec![],
        WorkContext::default(),
        LifecycleInput::default(),
    )?;
    let rows = store.project_memories(&reader.token, &project.project_id)?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].memory_id, corrected.memory_id);
    store.transfer_project(&owner.token, &project.project_id, &reader.account.person_id)?;
    let mut bundle = store.export_for_account(&owner.token)?;
    bundle.forgotten_ids.push(corrected.memory_id.clone());
    let _ = store.import_for_account(&owner.token, bundle);
    assert!(
        store
            .project_memories(&reader.token, &project.project_id)?
            .iter()
            .any(|m| m.memory_id == corrected.memory_id),
        "a former owner cannot hide another author's decision through import"
    );
    store.delete_for_account(&author.token, &corrected.memory_id)?;
    std::fs::remove_file(path)?;
    Ok(())
}

#[test]
fn captures_without_project_file_never_create_projects() -> Result<(), StoreError> {
    let path = temp_store_path("no-auto-project");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    store.capture_for_account(
        &owner.token,
        crate::test_support::observation("capture", "private repo capture"),
    )?;
    assert!(store.projects(&owner.token)?.is_empty());
    std::fs::remove_file(path)?;
    Ok(())
}

#[test]
fn recall_ranks_global_repo_and_project_together_without_private_cross_person_rows()
-> Result<(), StoreError> {
    let path = temp_store_path("mixed-recall");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
    let member = store.register_account("m@example.test", "correct horse battery staple")?;
    let project = store.register_project(&owner.token, "repo", "Repo", None)?;
    store.join_project(&member.token, &project.project_id)?;
    let ctx = WorkContext {
        repository: Some("repo".into()),
        ..WorkContext::default()
    };
    let source = store.remember_for_account(
        &owner.token,
        "mixneedle mixneedle mixneedle project",
        vec![],
        ctx.clone(),
        LifecycleInput::default(),
        MemoryKind::Explicit,
    )?;
    let decision = store.promote_memory(
        &owner.token,
        &project.project_id,
        &source.memory_id,
        "publish",
    )?;
    let global = store.remember_for_account(
        &member.token,
        "mixneedle global preference",
        vec![],
        WorkContext::default(),
        LifecycleInput::default(),
        MemoryKind::Lesson,
    )?;
    let own = store.remember_for_account(
        &member.token,
        "mixneedle repo note",
        vec![],
        WorkContext {
            branch: Some("feature".into()),
            worktree: Some("/private/worktree".into()),
            ..ctx.clone()
        },
        LifecycleInput::default(),
        MemoryKind::Explicit,
    )?;
    let unrelated = store.remember_for_account(
        &member.token,
        "mixneedle other repo",
        vec![],
        WorkContext {
            repository: Some("elsewhere".into()),
            ..WorkContext::default()
        },
        LifecycleInput::default(),
        MemoryKind::Explicit,
    )?;
    let filter = RecallFilter {
        session_id: None,
        context: Some(WorkContext {
            project_id: Some(project.project_id.clone()),
            ..ctx.clone()
        }),
    };
    let hits = store.search_for_account(&member.token, "mixneedle", 10, &filter)?;
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0].memory_id, decision.memory_id);
    assert_eq!(hits[0].source, "project");
    assert!(
        hits.iter()
            .any(|r| r.memory_id == global.memory_id && r.source == "personal")
    );
    assert!(
        hits.iter()
            .any(|r| r.memory_id == own.memory_id && r.source == "personal")
    );
    assert!(
        !hits
            .iter()
            .any(|r| r.memory_id == source.memory_id || r.memory_id == unrelated.memory_id)
    );
    let same_name_wrong_id = RecallFilter {
        session_id: None,
        context: Some(WorkContext {
            project_id: Some("wrong-project".into()),
            ..ctx
        }),
    };
    assert!(
        !store
            .search_for_account(&member.token, "mixneedle", 10, &same_name_wrong_id)?
            .iter()
            .any(|r| r.source == "project")
    );
    assert_eq!(
        store
            .context_for_account(&member.token, "mixneedle", 10, &filter)?
            .results
            .len(),
        3
    );
    assert_eq!(
        store
            .why_changed_for_account(&member.token, "mixneedle", 10, &filter)?
            .results
            .len(),
        3
    );
    std::fs::remove_file(path)?;
    Ok(())
}

#[test]
fn promotion_preview_is_full_read_only_and_secret_sources_are_refused() -> Result<(), StoreError> {
    let path = temp_store_path("preview-secret");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
    let project = store.register_project(&owner.token, "cairn-demo", "Demo", None)?;
    let source = remember_with_context(&mut store, &owner.token, &"full payload ".repeat(200))?;
    let before = std::fs::read(&path)?;
    let preview = store.preview_promotion(
        &owner.token,
        &project.project_id,
        &source.memory_id,
        "release",
    )?;
    assert_eq!(preview.content, source.content);
    assert_eq!(preview.context.files, source.context.files);
    assert_eq!(preview.context.symbols, source.context.symbols);
    assert!(preview.context.branch.is_none());
    assert_eq!(preview.promoted_from, None);
    assert!(preview.source_event_ids.is_empty());
    assert_eq!(std::fs::read(&path)?, before);
    assert!(
        store
            .project_memories(&owner.token, &project.project_id)?
            .is_empty()
    );
    let secret = store.remember_for_account(
        &owner.token,
        "secret material",
        vec![],
        context(),
        LifecycleInput {
            sensitivity: Some(Sensitivity::Secret),
            ..LifecycleInput::default()
        },
        MemoryKind::Explicit,
    )?;
    assert!(
        store
            .preview_promotion(
                &owner.token,
                &project.project_id,
                &secret.memory_id,
                "release"
            )
            .is_err()
    );
    assert!(
        store
            .promote_memory(
                &owner.token,
                &project.project_id,
                &secret.memory_id,
                "release"
            )
            .is_err()
    );
    let global = store.remember_for_account(
        &owner.token,
        "personal preference",
        vec![],
        WorkContext::default(),
        LifecycleInput::default(),
        MemoryKind::Lesson,
    )?;
    assert!(
        store
            .preview_promotion(
                &owner.token,
                &project.project_id,
                &global.memory_id,
                "release"
            )
            .is_err()
    );
    assert!(
        store
            .promote_memory(
                &owner.token,
                &project.project_id,
                &global.memory_id,
                "release"
            )
            .is_err()
    );
    std::fs::remove_file(path)?;
    Ok(())
}

#[test]
fn owner_and_admin_transfer_only_to_members_and_keep_authorship_and_private_branches()
-> Result<(), StoreError> {
    let path = temp_store_path("owner-transfer");
    let mut store = MemoryStore::open(&path)?;
    let admin = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
    let owner = store.register_account("o@example.test", "correct horse battery staple")?;
    let member = store.register_account("m@example.test", "correct horse battery staple")?;
    let outsider = store.register_account("x@example.test", "correct horse battery staple")?;
    let (project, decision) = owned_project(&mut store, &owner.token, "transfer decision")?;
    store.join_project(&member.token, &project)?;
    let branch = store.register_branch(&owner.token, &project, "feature", None, None, None)?;
    assert!(
        store
            .transfer_project(&outsider.token, &project, &member.account.person_id)
            .is_err()
    );
    assert!(
        store
            .transfer_project(&owner.token, &project, &outsider.account.person_id)
            .is_err()
    );
    store.verify_project_memory(
        &owner.token,
        &project,
        &decision.memory_id,
        "transfer-check",
    )?;
    store.transfer_project(&owner.token, &project, &member.account.person_id)?;
    let entities = store.project_entities(&member.token, &project)?;
    assert!(!entities.is_empty());
    let entity = &entities[0];
    let repeated = store.register_project_entity(
        &member.token,
        &project,
        ProjectEntityInput {
            kind: entity.kind.clone(),
            key: entity.key.clone(),
            content: None,
            source_memory_id: None,
            commit: None,
        },
    )?;
    assert_eq!(repeated.entity_id, entity.entity_id);
    assert!(!store.freshness_history(&member.token, &project)?.is_empty());
    assert_eq!(
        store.project_people(&owner.token, &project)?[0].person_id,
        member.account.person_id
    );
    assert_eq!(
        store.project_memories(&member.token, &project)?[0].published_by,
        Some(owner.account.person_id.clone())
    );
    assert_eq!(
        store.branches(&owner.token, &project)?[0].branch_id,
        branch.branch_id
    );
    assert!(store.branches(&member.token, &project)?.is_empty());
    store.verify_project_memory(&member.token, &project, &decision.memory_id, "new")?;
    store.transfer_project(&admin.token, &project, &owner.account.person_id)?;
    assert_eq!(
        store.project_people(&owner.token, &project)?[0].person_id,
        owner.account.person_id
    );
    assert_eq!(
        store
            .audit_events(&admin.token, 20)?
            .iter()
            .filter(|a| a.action == "project_owner_transferred")
            .count(),
        1
    );
    std::fs::remove_file(path)?;
    Ok(())
}

#[test]
fn member_branches_are_personal_and_devices_may_record_merges() -> Result<(), StoreError> {
    let path = temp_store_path("personal-merge");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
    let member = store.register_account("m@example.test", "correct horse battery staple")?;
    let project = store.register_project(&owner.token, "cairn-demo", "Demo", None)?;
    store.join_project(&member.token, &project.project_id)?;
    let first = store.register_branch(
        &owner.token,
        &project.project_id,
        "feat/auth",
        None,
        None,
        None,
    )?;
    let second = store.register_branch(
        &member.token,
        &project.project_id,
        "feat/auth",
        None,
        None,
        None,
    )?;
    assert_ne!(first.branch_id, second.branch_id);
    assert!(
        store
            .close_branch(&owner.token, &project.project_id, &second.branch_id, "no")
            .is_err()
    );
    let source = remember_with_context(&mut store, &member.token, "member branch evidence")?;
    let device = store.register_device("member-device", &member.account.person_id, None, "Hook")?;
    let merged = store.confirm_branch_merge(
        &device.token,
        &project.project_id,
        &second.branch_id,
        &source.memory_id,
        "merge123",
        "record merge",
    )?;
    assert_eq!(merged.branch.status, "merged");
    assert!(merged.memory.published_by.is_some());
    assert_eq!(
        store.branches(&owner.token, &project.project_id)?[0].status,
        "active"
    );
    std::fs::remove_file(path)?;
    Ok(())
}

#[test]
fn failed_project_attachment_warns_but_retains_personal_capture_and_retries()
-> Result<(), StoreError> {
    let path = temp_store_path("attachment-warning");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
    let mut input =
        crate::test_support::observation("attachment-event", "personal retained capture");
    input.context.project_id = Some("project-missing".into());
    let captured = store.capture_for_account(&owner.token, input.clone())?;
    let status = captured.project_attachment.expect("attachment status");
    assert_eq!(status.status, "personal");
    assert!(status.warning.is_some());
    assert!(store.projects(&owner.token)?.is_empty());
    assert_eq!(store.observations().count(), 1);
    let mut initialized = store.register_project(&owner.token, "cairn-demo", "Demo", None)?;
    store.state.project_owners[0].project_id = "project-missing".into();
    store.state.projects[0].project_id = "project-missing".into();
    initialized.project_id = "project-missing".into();
    store.persist()?;
    let retry = store.capture_for_account(&owner.token, input)?;
    assert!(retry.duplicate);
    assert_eq!(
        retry.project_attachment.expect("retried status").status,
        "attached"
    );
    assert_eq!(
        store.projects(&owner.token)?[0].project_id,
        initialized.project_id
    );
    std::fs::remove_file(path)?;
    Ok(())
}

#[test]
fn personal_promotion_cannot_supersede_another_authors_decision() -> Result<(), StoreError> {
    let path = temp_store_path("promotion-edit-rights");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
    let member = store.register_account("m@example.test", "correct horse battery staple")?;
    let project = store.register_project(&owner.token, "cairn-demo", "Demo", None)?;
    store.join_project(&member.token, &project.project_id)?;
    let source = remember_with_context(&mut store, &owner.token, "original author's source")?;
    let original = store.promote_memory(
        &owner.token,
        &project.project_id,
        &source.memory_id,
        "publish",
    )?;
    let mut forged = crate::test_support::linked_memory("forged-supersession", &source.memory_id);
    forged.context = context();
    forged.scope = MemoryScope::Project;
    crate::test_support::import_memories(&mut store, &member.token, vec![forged.clone()])?;
    let other = store.promote_memory(
        &member.token,
        &project.project_id,
        &forged.memory_id,
        "new independent decision",
    )?;
    assert!(other.supersedes.is_none());
    assert_eq!(
        store
            .project_memories(&owner.token, &project.project_id)?
            .len(),
        2
    );
    let correction = store.correct_for_account(
        &owner.token,
        &source.memory_id,
        "author corrected source",
        vec![],
        WorkContext::default(),
        LifecycleInput::default(),
    )?;
    assert_eq!(
        store
            .project_memories(&member.token, &project.project_id)?
            .len(),
        2
    );
    let replacement = store.promote_memory(
        &owner.token,
        &project.project_id,
        &correction.memory_id,
        "explicitly republish",
    )?;
    assert_eq!(
        replacement.supersedes.as_deref(),
        Some(original.memory_id.as_str())
    );
    let live = store.project_memories(&member.token, &project.project_id)?;
    assert_eq!(live.len(), 2);
    assert!(live.iter().all(|m| m.memory_id != original.memory_id));
    std::fs::remove_file(path)?;
    Ok(())
}

#[test]
fn promotion_precondition_refuses_a_summary_that_changed_after_preview() -> Result<(), StoreError> {
    let path = temp_store_path("changed-preview");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
    let project = store.register_project(&owner.token, "cairn-demo", "Demo", None)?;
    store.start_session_for_account(&owner.token, "preview-session", context())?;
    store.capture_for_account(
        &owner.token,
        crate::test_support::observation_for("preview-session", "first", "first payload"),
    )?;
    let source = store.end_session_for_account(&owner.token, "preview-session")?;
    let preview = store.preview_promotion(
        &owner.token,
        &project.project_id,
        &source.memory_id,
        "publish",
    )?;
    let fingerprint = publication_fingerprint(&preview)?;
    store.start_session_for_account(&owner.token, "preview-session", context())?;
    store.capture_for_account(
        &owner.token,
        crate::test_support::observation_for("preview-session", "second", "changed payload"),
    )?;
    store.end_session_for_account(&owner.token, "preview-session")?;
    assert!(matches!(
        store.promote_memory_checked(
            &owner.token,
            &project.project_id,
            &source.memory_id,
            "publish",
            Some(&fingerprint)
        ),
        Err(StoreError::Conflict(_))
    ));
    assert!(
        store
            .project_memories(&owner.token, &project.project_id)?
            .is_empty()
    );
    let updated = store.preview_promotion(
        &owner.token,
        &project.project_id,
        &source.memory_id,
        "publish",
    )?;
    let accepted = store.promote_memory_checked(
        &owner.token,
        &project.project_id,
        &source.memory_id,
        "publish",
        Some(&publication_fingerprint(&updated)?),
    )?;
    assert_eq!(accepted.content, updated.content);
    std::fs::remove_file(path)?;
    Ok(())
}

#[test]
fn remote_rename_keeps_tagged_personal_memories_promotable_by_project_id() -> Result<(), StoreError>
{
    let path = temp_store_path("remote-rename");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("rename@example.test", "correct horse battery staple")?;
    let project = store.register_project(&owner.token, "host/org/old", "Repo", None)?;
    let source = store.remember_for_account(
        &owner.token,
        "renameneedle keep the project identity",
        vec![],
        WorkContext {
            repository: Some("host/org/renamed".into()),
            project_id: Some(project.project_id.clone()),
            ..WorkContext::default()
        },
        LifecycleInput::default(),
        MemoryKind::Explicit,
    )?;
    assert_eq!(
        store
            .personal_project_memories(&owner.token, &project.project_id)?
            .len(),
        1
    );
    store.preview_promotion(
        &owner.token,
        &project.project_id,
        &source.memory_id,
        "rename",
    )?;
    store.promote_memory(
        &owner.token,
        &project.project_id,
        &source.memory_id,
        "rename",
    )?;
    let hits = store.search_for_account(
        &owner.token,
        "renameneedle",
        10,
        &RecallFilter {
            context: Some(WorkContext {
                repository: Some("host/org/renamed".into()),
                project_id: Some(project.project_id.clone()),
                ..WorkContext::default()
            }),
            ..RecallFilter::default()
        },
    )?;
    assert!(hits.iter().any(|r| r.source == "project"));
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn rename_is_owner_only_audited_and_persisted_with_rollback_on_write_failure()
-> Result<(), StoreError> {
    let dir = temp_store_dir("rename-project");
    let path = dir.join("store.json");
    let mut store = MemoryStore::open(&path)?;
    let owner =
        store.bootstrap_account("rename-owner@example.test", "correct horse battery staple")?;
    let member =
        store.register_account("rename-member@example.test", "correct horse battery staple")?;
    let outsider = store.register_account(
        "rename-outsider@example.test",
        "correct horse battery staple",
    )?;
    let (project, _) = store.init_project(&owner.token, "repo", Some("--help"))?;
    assert!(
        store
            .join_project_named(&member.token, &project.project_id, Some("Wrong"))
            .is_err()
    );
    assert!(
        store.projects(&member.token)?.is_empty(),
        "a name mismatch must not join"
    );
    store.join_project(&member.token, &project.project_id)?;
    for token in [&member.token, &outsider.token] {
        assert!(
            store
                .rename_project(token, &project.project_id, "No")
                .is_err()
        );
    }
    for name in [" ".to_owned(), "x".repeat(513)] {
        assert!(
            store
                .rename_project(&owner.token, &project.project_id, &name)
                .is_err()
        );
    }
    let renamed = store.rename_project(&owner.token, &project.project_id, "  Useful name  ")?;
    assert_eq!(renamed.name, "Useful name");
    assert_eq!(renamed.project_id, project.project_id);
    let audits = store.audit_events(&owner.token, 100)?;
    let audit = audits
        .iter()
        .find(|event| event.action == "project_renamed")
        .unwrap();
    assert_eq!(
        audit.subject_id.as_deref(),
        Some(project.project_id.as_str())
    );
    assert_eq!(
        audit.metadata,
        serde_json::json!({"previous_name":"--help", "name":"Useful name"})
    );
    store.rename_project(&owner.token, &project.project_id, "Useful name")?;
    assert_eq!(
        store.audit_events(&owner.token, 100)?.len(),
        audits.len(),
        "same name is a no-op"
    );
    assert_eq!(
        MemoryStore::open(&path)?.projects(&member.token)?[0].name,
        "Useful name"
    );
    // Force persist's rename to fail after the new name and audit were staged.
    fs::remove_file(&path)?;
    fs::create_dir(&path)?;
    assert!(
        store
            .rename_project(&owner.token, &project.project_id, "Failed")
            .is_err()
    );
    assert_eq!(store.projects(&owner.token)?[0].name, "Useful name");
    assert_eq!(store.audit_events(&owner.token, 100)?.len(), audits.len());
    fs::remove_dir_all(dir)?;
    Ok(())
}
