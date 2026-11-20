use crate::MemoryStatus;
use crate::lifecycle::{LifecycleInput, MemoryKind, Retention};
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

fn save_note(
    store: &mut MemoryStore,
    token: &str,
    title: &str,
    evidence: &[&str],
) -> Result<crate::Memory, StoreError> {
    store.save_for_account(
        token,
        serde_json::from_value(serde_json::json!({
            "scope": "personal",
            "kind": "fact",
            "title": title,
            "summary": title,
            "reason": "verified",
            "statement": title,
            "evidence": evidence
        }))
        .unwrap(),
    )
}

#[test]
fn identical_explicit_save_reuses_one_record_and_merges_evidence() -> Result<(), StoreError> {
    let path = temp_store_path("save-dedupe");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@dedupe.test", "correct horse battery staple")?;
    let other = store.register_account("other@dedupe.test", "correct horse battery staple")?;

    let first = save_note(&mut store, &owner.token, "dedupe-note", &["docs/a"])?;
    let second = save_note(&mut store, &owner.token, "dedupe-note", &["docs/b"])?;
    assert_eq!(first.memory_id, second.memory_id);
    assert_eq!(second.fields.evidence, vec!["docs/a", "docs/b"]);
    assert_eq!(second.source_event_ids, first.source_event_ids);
    let changed_at = second.changed_at.clone();
    let third = save_note(&mut store, &owner.token, "dedupe-note", &["docs/b"])?;
    assert_eq!(third.memory_id, first.memory_id);
    assert_eq!(
        third.changed_at, changed_at,
        "a no-op retry must not rewrite"
    );
    assert_eq!(
        store.memories_for_account(&owner.token)?.len(),
        1,
        "only one record survives"
    );

    let different_text = store.save_for_account(
        &owner.token,
        serde_json::from_value(serde_json::json!({
            "scope": "personal",
            "kind": "fact",
            "title": "dedupe-note",
            "summary": "different summary",
            "reason": "verified",
            "statement": "dedupe-note"
        }))
        .unwrap(),
    )?;
    assert_ne!(different_text.memory_id, first.memory_id);

    let different_kind = store.save_for_account(
        &owner.token,
        serde_json::from_value(serde_json::json!({
            "scope": "personal",
            "kind": "rule",
            "title": "dedupe-note",
            "summary": "dedupe-note",
            "reason": "verified",
            "rule": "keep"
        }))
        .unwrap(),
    )?;
    assert_ne!(different_kind.memory_id, first.memory_id);

    let different_scope = store.save_for_account(
        &owner.token,
        serde_json::from_value(serde_json::json!({
            "scope": "global",
            "kind": "fact",
            "title": "dedupe-note",
            "summary": "dedupe-note",
            "reason": "verified",
            "statement": "dedupe-note"
        }))
        .unwrap(),
    )?;
    assert_ne!(different_scope.memory_id, first.memory_id);

    let durable = |expires_at: &str| {
        serde_json::from_value(serde_json::json!({
            "scope": "personal",
            "kind": "fact",
            "title": "expiry-note",
            "summary": "expiry-note",
            "reason": "verified",
            "statement": "expiry-note",
            "lifecycle": {"retention": "durable", "expiresAt": expires_at}
        }))
        .unwrap()
    };
    let first_expiry = store.save_for_account(&owner.token, durable("4102444800"))?;
    let other_expiry = store.save_for_account(&owner.token, durable("4102444801"))?;
    assert_ne!(first_expiry.memory_id, other_expiry.memory_id);
    let same_expiry = store.save_for_account(&owner.token, durable("4102444800"))?;
    assert_eq!(same_expiry.memory_id, first_expiry.memory_id);

    let other_owner = save_note(&mut store, &other.token, "dedupe-note", &[])?;
    assert_ne!(other_owner.memory_id, first.memory_id);
    assert!(
        store
            .memories_for_account(&other.token)?
            .iter()
            .all(|memory| memory.memory_id != first.memory_id)
    );

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn identical_save_skips_forgotten_expired_and_superseded_records() -> Result<(), StoreError> {
    let path = temp_store_path("save-dedupe-lifecycle");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@lifecycle.test", "correct horse battery staple")?;

    let forgotten = save_note(&mut store, &owner.token, "lifecycle-note", &[])?;
    store.forget_for_account(&owner.token, &forgotten.memory_id)?;
    let after_forget = save_note(&mut store, &owner.token, "lifecycle-note", &[])?;
    assert_ne!(after_forget.memory_id, forgotten.memory_id);

    let expired = save_note(&mut store, &owner.token, "expired-note", &[])?;
    let row = store
        .state
        .memories
        .iter_mut()
        .find(|memory| memory.memory_id == expired.memory_id)
        .unwrap();
    row.lifecycle.expires_at = Some("1".to_owned());
    store.persist()?;
    let after_expiry = save_note(&mut store, &owner.token, "expired-note", &[])?;
    assert_ne!(after_expiry.memory_id, expired.memory_id);

    let superseded = save_note(&mut store, &owner.token, "superseded-note", &[])?;
    let row = store
        .state
        .memories
        .iter_mut()
        .find(|memory| memory.memory_id == superseded.memory_id)
        .unwrap();
    row.fields.status = MemoryStatus::Superseded;
    store.persist()?;
    let after_status = save_note(&mut store, &owner.token, "superseded-note", &[])?;
    assert_ne!(after_status.memory_id, superseded.memory_id);

    let hidden = save_note(&mut store, &owner.token, "hidden-note", &[])?;
    let successor = store.save_for_account(
        &owner.token,
        serde_json::from_value(serde_json::json!({
            "scope": "personal",
            "kind": "fact",
            "title": "hidden-note",
            "summary": "successor",
            "reason": "verified",
            "statement": "successor",
            "supersedes": hidden.memory_id
        }))
        .unwrap(),
    )?;
    assert_ne!(successor.memory_id, hidden.memory_id);
    let after_successor = save_note(&mut store, &owner.token, "hidden-note", &[])?;
    assert_ne!(after_successor.memory_id, hidden.memory_id);

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn temporary_explicit_save_retry_reuses_one_record() -> Result<(), StoreError> {
    let path = temp_store_path("save-dedupe-temporary");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@temporary.test", "correct horse battery staple")?;
    let input = || {
        serde_json::from_value(serde_json::json!({
            "scope": "personal",
            "kind": "fact",
            "title": "temporary-note",
            "summary": "temporary-note",
            "reason": "verified",
            "statement": "temporary-note",
            "lifecycle": {"retention": "temporary"}
        }))
        .unwrap()
    };
    let first = store.save_for_account(&owner.token, input())?;
    assert_eq!(first.lifecycle.retention, Retention::Temporary);
    assert!(first.lifecycle.expires_at.is_some());
    let second = store.save_for_account(&owner.token, input())?;
    assert_eq!(first.memory_id, second.memory_id);
    assert_eq!(store.memories_for_account(&owner.token)?.len(), 1);

    fs::remove_file(path)?;
    Ok(())
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
