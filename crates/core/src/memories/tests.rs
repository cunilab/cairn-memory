use crate::lifecycle::{LifecycleInput, MemoryKind};
use crate::recall::RecallFilter;
use crate::store::MemoryStore;
use crate::test_support::{HeldStoreDir, context, remember_owned, temp_store_dir, temp_store_path};
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
fn structured_save_requires_scope_title_summary_reason_and_preserves_type_fields() {
    let path = temp_store_path("structured-save");
    let mut store = MemoryStore::open(&path).unwrap();
    let admin = store
        .bootstrap_account("admin@shape.test", "correct horse battery staple")
        .unwrap();
    for field in ["scope", "title", "summary", "reason"] {
        let mut value = serde_json::json!({"scope":"personal", "title":"Title", "summary":"Summary", "reason":"Reason"});
        value.as_object_mut().unwrap().remove(field);
        let input = serde_json::from_value(value).unwrap();
        let error = store.save_for_account(&admin.token, input).unwrap_err();
        assert!(error.to_string().contains(field), "{error}");
    }
    for (kind, fields) in [
        (
            "decision",
            serde_json::json!({"decision_context":"constraints", "alternatives":["other"], "consequences":"tradeoff"}),
        ),
        (
            "rule",
            serde_json::json!({"rule":"borrow", "exceptions":"owned transfer"}),
        ),
        (
            "gotcha",
            serde_json::json!({"symptom":"failure", "cause":"race", "fix":"lock"}),
        ),
        (
            "procedure",
            serde_json::json!({"steps":["first","second"], "when":"release"}),
        ),
        (
            "fact",
            serde_json::json!({"statement":"verified", "source":"check"}),
        ),
        (
            "preference",
            serde_json::json!({"preference":"concise", "example":"one paragraph"}),
        ),
    ] {
        let mut input = serde_json::json!({"scope":"personal", "title":"Title", "summary":"Summary", "reason":"Reason", "kind":kind, "details":"Body", "applies_to":["src/**"], "evidence":["cargo test"], "tags":["rust"], "entities":["Memory"], "pinned":true, "importance":90, "status":"active"});
        input
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let memory = store
            .save_for_account(&admin.token, serde_json::from_value(input.clone()).unwrap())
            .unwrap();
        let output = serde_json::to_value(memory).unwrap();
        for (key, value) in input.as_object().unwrap() {
            assert_eq!(&output[key], value, "{kind}.{key}");
        }
    }
    drop(store);
    let mut store = MemoryStore::open(&path).unwrap();
    assert_eq!(
        store
            .search_for_account(&admin.token, "Reason", 20, &RecallFilter::default())
            .unwrap()
            .len(),
        6
    );
    let _ = fs::remove_file(path);
}

#[test]
fn record_title_has_highest_lexical_weight() {
    let path = temp_store_path("title-weight");
    let mut store = MemoryStore::open(&path).unwrap();
    let account = store
        .bootstrap_account("rank@shape.test", "correct horse battery staple")
        .unwrap();
    for (title, summary) in [("Ordinary", "weightneedle"), ("weightneedle", "Ordinary")] {
        store.save_for_account(&account.token,serde_json::from_value(serde_json::json!({"scope":"personal","title":title,"summary":summary,"reason":"verified"})).unwrap()).unwrap();
    }
    let rows = store
        .search_for_account(&account.token, "weightneedle", 2, &RecallFilter::default())
        .unwrap();
    assert_eq!(rows[0].fields.title, "weightneedle");
    fs::remove_file(path).unwrap();
}
