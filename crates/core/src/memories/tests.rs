use crate::lifecycle::{LifecycleInput, MemoryKind};
use crate::recall::RecallFilter;
use crate::store::MemoryStore;
use crate::test_support::{
    HeldStoreDir, context, owned_project, remember_owned, temp_store_dir, temp_store_path,
};
use crate::types::{StoreError, WorkContext};
use std::fs;

#[test]
fn failed_account_correction_persist_leaves_no_unowned_successor() -> Result<(), StoreError> {
    let dir = temp_store_dir("account-correct-commit");
    let path = dir.join("store.json");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    remember_owned(
        &mut store,
        &owner.token,
        "correctionalphafact stays recalled",
    )?;
    let target = store
        .search_for_account(
            &owner.token,
            "correctionalphafact",
            5,
            &RecallFilter::default(),
        )?
        .remove(0)
        .memory_id;
    let before = fs::read(&path)?;
    let memories_before = store.state.memories.len();
    let owners_before = store.state.memory_owners.len();
    let sources_before = store.state.observation_owners.len();

    let mut blocked = HeldStoreDir::block(&path);
    let failed = store
        .correct_for_account(
            &owner.token,
            &target,
            "correctionbetafact replaces it",
            Vec::new(),
            WorkContext::default(),
            LifecycleInput::default(),
        )
        .expect_err("a blocked store rename must fail the account correction");
    assert!(
        matches!(failed, StoreError::Io(_)),
        "storage error, got {failed}"
    );
    // An unowned successor would hide the owner's target from recall.
    assert_eq!(store.state.memories.len(), memories_before);
    assert_eq!(store.state.memory_owners.len(), owners_before);
    assert_eq!(store.state.observation_owners.len(), sources_before);
    let recalled = store.search_for_account(
        &owner.token,
        "correctionalphafact",
        5,
        &RecallFilter::default(),
    )?;
    assert_eq!(recalled.len(), 1);
    blocked.release();
    assert_eq!(fs::read(&path)?, before);

    let retry = store.correct_for_account(
        &owner.token,
        &target,
        "correctionbetafact replaces it",
        Vec::new(),
        WorkContext::default(),
        LifecycleInput::default(),
    )?;
    assert!(store.memory_owned_by(&retry.memory_id, &owner.account.person_id));
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn lessons_and_corrections_preserve_kind_and_supersession() -> Result<(), StoreError> {
    let path = temp_store_path("correction");
    let mut store = MemoryStore::open(&path)?;
    let lesson = store.remember_lesson(
        "Do not replay provider calls during import.",
        vec!["portability".to_owned()],
        context(),
        LifecycleInput::default(),
    )?;
    assert_eq!(lesson.kind, MemoryKind::Lesson);

    let correction = store.correct(
        &lesson.memory_id,
        "Do not replay provider calls during import; preserve the local tombstone.",
        Vec::new(),
        WorkContext::default(),
        LifecycleInput {
            confidence: Some(40),
            ..LifecycleInput::default()
        },
    )?;
    assert_eq!(correction.kind, MemoryKind::Correction);
    assert_eq!(correction.lifecycle.confidence, 100);
    assert_eq!(correction.supersedes, Some(lesson.memory_id.clone()));
    let corrected = store.search("local tombstone", 5)?;
    assert_eq!(corrected.len(), 1);
    assert_eq!(corrected[0].memory_id, correction.memory_id);
    let shared = store.search("replay provider calls", 5)?;
    assert_eq!(shared.len(), 1);
    assert_eq!(shared[0].memory_id, correction.memory_id);
    let exported = store.export();
    assert!(
        exported
            .memories
            .iter()
            .any(|memory| memory.memory_id == lesson.memory_id)
    );
    assert!(
        !exported
            .forgotten_ids
            .iter()
            .any(|id| id == &lesson.memory_id)
    );

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn correction_of_a_project_memory_keeps_project_id_and_hides_the_old_listing()
-> Result<(), StoreError> {
    let path = temp_store_path("correction-project");
    let mut store = MemoryStore::open(&path)?;
    let account = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let project =
        store.register_project(&account.token, "cairn", "Cairn", Some("main".to_owned()))?;
    let source = store.remember(
        "The cache decision passed.",
        Vec::new(),
        WorkContext {
            repository: Some("cairn".to_owned()),
            ..WorkContext::default()
        },
    )?;
    let promoted = store.promote_memory(
        &account.token,
        &project.project_id,
        &source.memory_id,
        "Confirmed by the cache regression test",
    )?;
    let correction = store.correct(
        &promoted.memory_id,
        "The cache decision passed after review.",
        Vec::new(),
        WorkContext::default(),
        LifecycleInput {
            confidence: Some(10),
            ..LifecycleInput::default()
        },
    )?;
    assert_eq!(
        correction.project_id.as_deref(),
        Some(project.project_id.as_str())
    );
    assert_eq!(correction.lifecycle.confidence, 100);
    let listed = store.project_memories(&account.token, &project.project_id)?;
    assert!(
        listed
            .iter()
            .any(|memory| memory.memory_id == correction.memory_id)
    );
    assert!(
        listed
            .iter()
            .all(|memory| memory.memory_id != promoted.memory_id)
    );
    assert!(
        store
            .export()
            .memories
            .iter()
            .any(|memory| memory.memory_id == promoted.memory_id)
    );
    let report = store.project_maintenance(&account.token, &project.project_id, None)?;
    assert!(
        report
            .superseded_memory_ids
            .iter()
            .any(|id| id == &promoted.memory_id)
    );
    assert!(
        report
            .active_memory_ids
            .iter()
            .any(|id| id == &correction.memory_id)
    );
    assert!(
        report
            .active_memory_ids
            .iter()
            .all(|id| id != &promoted.memory_id)
    );

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn promoting_a_correction_does_not_supersede_a_forgotten_copy() -> Result<(), StoreError> {
    let path = temp_store_path("prior-copy-live");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let (project_id, copy) = owned_project(&mut store, &owner.token, "priorcopy fact")?;
    let source_id = copy.promoted_from.clone().expect("origin");
    store.forget_for_account(&owner.token, &copy.memory_id)?;
    let correction = store.correct_for_account(
        &owner.token,
        &source_id,
        "priorcopy corrected fact",
        Vec::new(),
        context(),
        LifecycleInput::default(),
    )?;
    let promoted = store.promote_memory(&owner.token, &project_id, &correction.memory_id, "why")?;
    assert_eq!(
        promoted.supersedes, None,
        "a forgotten copy is not a predecessor"
    );
    fs::remove_file(path)?;
    Ok(())
}
