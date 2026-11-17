use crate::capture::{ObservationInput, validate_observation};
use crate::lifecycle::{Lifecycle, LifecycleInput, MemoryKind, Retention, Sensitivity};
use crate::memories::build_explicit_memory_record;
use crate::ownership::MemoryOwnerRecord;
use crate::recall::RecallFilter;
use crate::store::{MemoryStore, STORE_VERSION};
use crate::test_support::{
    HeldStoreDir, assert_unknown_id, context, ids_of, import_forgotten, linked_memory, observation,
    observation_for, remember_owned, temp_store_dir, temp_store_path,
};
use crate::transfer::normalize_export_bundle;
use crate::types::{StoreError, WorkContext, now_seconds};
use std::collections::HashSet;
use std::fs;

#[test]
fn account_forget_and_delete_require_an_owned_id() -> Result<(), StoreError> {
    let path = temp_store_path("forget-owned");
    let mut store = MemoryStore::open(&path)?;
    // Legacy rows written before accounts: an event, its summary and an explicit memory.
    store.start_session("legacy-sess", context())?;
    store.capture_observation(observation_for(
        "legacy-sess",
        "legacy-evt",
        "legacyobs fact",
    ))?;
    let summary = store.end_session("legacy-sess")?;
    let explicit = store.remember("legacymem note", Vec::new(), context())?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let other = store.register_account("other@example.test", "correct horse battery staple")?;
    let mine = remember_owned(&mut store, &owner.token, "forgetmine note")?;
    let before = fs::read(&path)?;

    // Unowned legacy, foreign and unknown ids all fail alike and write nothing.
    for id in [
        "legacy-evt",
        summary.memory_id.as_str(),
        explicit.memory_id.as_str(),
        mine.memory_id.as_str(),
        "no-such-id",
    ] {
        assert_unknown_id(&store.forget_for_account(&other.token, id).unwrap_err(), id);
        assert_unknown_id(&store.delete_for_account(&other.token, id).unwrap_err(), id);
    }
    assert_eq!(fs::read(&path)?, before, "nothing is written on rejection");
    assert!(store.state.forgotten_ids.is_empty());
    assert!(store.state.sync_deletions.is_empty());
    assert!(store.state.audit_events.is_empty());
    assert!(
        store
            .forget_for_account("not-a-token", "legacy-evt")
            .is_err()
    );

    // The trusted unscoped call still forgets legacy data; the owner forgets and deletes its own.
    store.forget("legacy-evt")?;
    assert!(store.state.forgotten_ids.contains("legacy-evt"));
    store.delete_for_account(&owner.token, &mine.memory_id)?;
    assert!(store.state.forgotten_ids.contains(&mine.memory_id));
    fs::remove_file(path)?;
    Ok(())
}

// Project actions never delete personal memory: the promoted copy keeps no source events,
// so forget and retained delete of the copy stop at the copy and its shares.
#[test]
fn forget_cascades_only_sources_the_caller_owns() -> Result<(), StoreError> {
    let dir = temp_store_dir("forget-cascade");
    let path = dir.join("store.json");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let other = store.register_account("other@example.test", "correct horse battery staple")?;
    let device = store.register_device("dev-cascade", &owner.account.person_id, None, "Owner")?;
    let filter = RecallFilter::default();
    store.start_session_for_account(&owner.token, "victim-sess", context())?;
    store.capture_for_account(
        &owner.token,
        observation_for("victim-sess", "victim-evt", "victimcascade fact"),
    )?;
    store.sync_push(
        &device.token,
        observation_for("victim-sess", "victim-evt", "victimcascade fact"),
    )?;
    let summary = store.end_session_for_account(&owner.token, "victim-sess")?;
    let note = remember_owned(&mut store, &owner.token, "victimnote explicit")?;

    // Finding 3: the importer's own row names a foreign event, a foreign memory and a
    // future event as sources. Forgetting that row must not tombstone any of them.
    let mut row = linked_memory("cascade", "memory-unrelated");
    row.supersedes = None;
    row.source_event_ids = vec![
        "victim-evt".to_owned(),
        note.memory_id.clone(),
        "future-evt".to_owned(),
    ];
    import_forgotten(&mut store, &other.token, vec![row], &[])?;
    store.forget_for_account(&other.token, "memory-link-cascade")?;
    let expected = HashSet::from(["memory-link-cascade".to_owned()]);
    assert_eq!(store.state.forgotten_ids, expected);
    assert!(
        store
            .state
            .sync_deletions
            .iter()
            .all(|deletion| deletion.item_id == "memory-link-cascade")
    );
    let hits = store.search_for_account(&owner.token, "victimnote", 10, &filter)?;
    assert_eq!(ids_of(&hits), vec![note.memory_id.as_str()]);
    let resume = store.resume_for_account(&owner.token, 10, &filter)?;
    assert!(ids_of(&resume.results).contains(&summary.memory_id.as_str()));
    assert_eq!(store.sync_pull(&device.token, 0)?.events.len(), 1);
    store.start_session_for_account(&owner.token, "later-sess", context())?;
    store.capture_for_account(
        &owner.token,
        observation_for("later-sess", "future-evt", "latercascade fact"),
    )?;
    assert!(
        store
            .end_session_for_account(&owner.token, "later-sess")?
            .content
            .contains("latercascade")
    );
    assert_eq!(MemoryStore::open(&path)?.state.forgotten_ids, expected);

    // Own sources still cascade: a real owned observation and the generated explicit source.
    let generated = note.source_event_ids[0].clone();
    store.forget_for_account(&owner.token, &summary.memory_id)?;
    assert!(store.state.forgotten_ids.contains("victim-evt"));
    store.forget_for_account(&owner.token, &note.memory_id)?;
    assert!(store.state.forgotten_ids.contains(&generated));
    let reopened = MemoryStore::open(&path)?;
    assert!(reopened.state.forgotten_ids.contains(&generated));

    // Old data: an owned memory whose generated source has no owner record. The memory is
    // forgotten by its own id; the source is neither tombstoned nor claimed.
    let mut old = linked_memory("old", "memory-unrelated");
    old.supersedes = None;
    old.source_event_ids = vec!["event-old-source".to_owned()];
    store.state.memories.push(old);
    store.state.memory_owners.push(MemoryOwnerRecord {
        memory_id: "memory-link-old".to_owned(),
        person_id: owner.account.person_id.clone(),
    });
    store.persist()?;
    store.forget_for_account(&owner.token, "memory-link-old")?;
    assert!(store.state.forgotten_ids.contains("memory-link-old"));
    assert!(!store.state.forgotten_ids.contains("event-old-source"));
    assert!(
        !store
            .state
            .observation_owners
            .iter()
            .any(|r| r.event_id == "event-old-source")
    );
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn generated_explicit_source_is_owned_without_a_row_and_rolls_back() -> Result<(), StoreError> {
    let dir = temp_store_dir("generated-source");
    let path = dir.join("store.json");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let other = store.register_account("other@example.test", "correct horse battery staple")?;
    let device = store.register_device("dev-gen", &other.account.person_id, None, "Other")?;
    let note = remember_owned(&mut store, &owner.token, "generated source note")?;
    let source = note.source_event_ids[0].clone();
    assert!(store.observation_owned_by(&source, &owner.account.person_id));
    assert!(
        store.observations().all(|o| o.event_id != source),
        "no fake observation row"
    );
    assert!(MemoryStore::open(&path)?.observation_owned_by(&source, &owner.account.person_id));

    // The owned id cannot be taken by another account's capture, device push or import.
    store.start_session_for_account(&other.token, "gen-sess", context())?;
    let input = observation_for("gen-sess", &source, "squat attempt");
    let expected = format!("event belongs to another person: {source}");
    for error in [
        store
            .capture_for_account(&other.token, input.clone())
            .unwrap_err(),
        store.sync_push(&device.token, input).unwrap_err(),
    ] {
        assert!(matches!(&error, StoreError::Validation(text) if text == &expected));
    }
    assert!(store.observations().all(|o| o.event_id != source));
    let mut row = linked_memory("gen", "memory-unrelated");
    row.supersedes = None;
    row.memory_id = source.clone();
    assert!(import_forgotten(&mut store, &other.token, vec![row], &[]).is_err());
    assert!(
        store
            .export_for_account(&owner.token)?
            .observations
            .is_empty()
    );

    // A failed rename leaves neither the memory nor its owner records.
    let before = fs::read(&path)?;
    let owners = (
        store.state.memory_owners.len(),
        store.state.observation_owners.len(),
    );
    let mut blocked = HeldStoreDir::block(&path);
    let failed = store
        .remember_for_account(
            &owner.token,
            "blocked note",
            Vec::new(),
            context(),
            LifecycleInput::default(),
            MemoryKind::Explicit,
        )
        .expect_err("a blocked rename fails the remember");
    assert!(matches!(failed, StoreError::Io(_)));
    assert_eq!(
        (
            store.state.memory_owners.len(),
            store.state.observation_owners.len()
        ),
        owners
    );
    assert!(
        store
            .state
            .memories
            .iter()
            .all(|m| m.content != "blocked note")
    );
    blocked.release();
    assert_eq!(fs::read(&path)?, before);
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn durable_summary_survives_raw_observation_expiry_until_forget() -> Result<(), StoreError> {
    let path = temp_store_path("durable-expiry");
    let mut store = MemoryStore::open(&path)?;
    store.start_session("durable-session", WorkContext::default())?;
    store.capture_observation(ObservationInput {
        event_id: "raw-evt".to_owned(),
        session_id: "durable-session".to_owned(),
        content: "durable summary source evidence".to_owned(),
        role: "agent".to_owned(),
        source: "test".to_owned(),
        importance: None,
        lifecycle: LifecycleInput {
            retention: Some(Retention::Temporary),
            expires_at: Some((now_seconds() + 3600).to_string()),
            ..LifecycleInput::default()
        },
        context: WorkContext::default(),
    })?;
    let summary = store.end_session("durable-session")?;
    assert_eq!(summary.lifecycle.retention, Retention::Durable);
    store
        .state
        .observations
        .iter_mut()
        .find(|observation| observation.event_id == "raw-evt")
        .unwrap()
        .lifecycle
        .expires_at = Some((now_seconds() - 1).to_string());
    store.sweep_expired()?;
    let recalled = store.search("durable summary source evidence", 5)?;
    assert_eq!(
        recalled.len(),
        2,
        "session note and separate handoff survive raw expiry"
    );
    assert!(recalled.iter().any(|m| m.memory_id == summary.memory_id));
    store.forget(&summary.memory_id)?;
    assert!(
        store
            .search("durable summary source evidence", 5)?
            .is_empty()
    );
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn invalid_observation_and_lifecycle_input_is_rejected_before_persistence() -> Result<(), StoreError>
{
    let path = temp_store_path("validation-boundaries");
    let mut store = MemoryStore::open(&path)?;

    let mut invalid = observation("invalid-empty-role", "should not persist");
    invalid.role.clear();
    assert!(store.capture_observation(invalid).is_err());

    let mut invalid = observation("invalid-importance", "should not persist");
    invalid.lifecycle.importance = Some(101);
    assert!(store.capture_observation(invalid).is_err());

    let mut invalid = observation("invalid-expiry", "should not persist");
    invalid.lifecycle.expires_at = Some("not-a-timestamp".to_owned());
    assert!(store.capture_observation(invalid).is_err());

    assert_eq!(store.observations().count(), 0);
    assert!(!path.exists());

    let _ = fs::remove_file(path);
    Ok(())
}

// The PostgreSQL read queries cast `lifecycle->>'expiresAt'` with `::bigint`, so every
// write path must accept exactly the values that cast accepts: 0..=i64::MAX.
#[test]
fn expiry_is_bounded_to_i64_unix_seconds_on_input_and_import_paths() -> Result<(), StoreError> {
    let path = temp_store_path("expiry-bound");
    let mut source = MemoryStore::open(&path)?;
    source.capture_observation(observation("expiry-bound-event", "expiryfact"))?;
    source.remember("expiryfact memory", Vec::new(), context())?;
    let bundle = source.export();
    fs::remove_file(path)?;

    let max = i64::MAX.to_string();
    for (value, valid) in [
        (max.as_str(), true),
        ("0009223372036854775807", true),
        ("0", true),
        ("+5", true),
        ("0000000005", true),
        ("9223372036854775808", false),
        ("0009223372036854775808", false),
        ("18446744073709551615", false),
        ("18446744073709551616", false),
        ("-1", false),
        ("", false),
        ("soon", false),
    ] {
        let lifecycle = LifecycleInput {
            expires_at: Some(value.to_owned()),
            ..LifecycleInput::default()
        };
        let mut input = observation("expiry-bound-input", "expiryfact");
        input.lifecycle = lifecycle.clone();
        let remembered = build_explicit_memory_record(
            "expiryfact",
            Vec::new(),
            WorkContext::default(),
            lifecycle,
            MemoryKind::Explicit,
        );
        let mut imported = bundle.clone();
        imported.observations[0].lifecycle.expires_at = Some(value.to_owned());
        imported.memories[0].lifecycle.expires_at = Some(value.to_owned());
        assert_eq!(
            validate_observation(&input).is_ok(),
            valid,
            "observation {value:?}"
        );
        assert_eq!(remembered.is_ok(), valid, "remember {value:?}");
        assert_eq!(
            normalize_export_bundle(&mut imported).is_ok(),
            valid,
            "import {value:?}"
        );
    }
    Ok(())
}

#[test]
fn forget_suppresses_derived_memory_after_restart() -> Result<(), StoreError> {
    let path = temp_store_path("forget");
    {
        let mut store = MemoryStore::open(&path)?;
        store.start_session("sess-auth-001", context())?;
        store.capture_observation(observation("evt-forget", "private debugging note"))?;
        store.end_session("sess-auth-001")?;
        store.forget("evt-forget")?;
        assert!(store.search("private debugging note", 5)?.is_empty());
    }

    let store = MemoryStore::open(&path)?;
    assert!(store.search("private debugging note", 5)?.is_empty());

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn classifies_lifecycle_dimensions_and_excludes_expired_memory() -> Result<(), StoreError> {
    let path = temp_store_path("lifecycle");
    let mut store = MemoryStore::open(&path)?;
    store.start_session("sess-lifecycle", context())?;
    let capture = store.capture_observation(ObservationInput {
        event_id: "evt-lifecycle".to_owned(),
        session_id: "sess-lifecycle".to_owned(),
        content: "Private release decision with password=redacted-value".to_owned(),
        role: "agent".to_owned(),
        source: "test".to_owned(),
        importance: Some("high".to_owned()),
        lifecycle: LifecycleInput {
            confidence: Some(90),
            freshness: Some(70),
            ..LifecycleInput::default()
        },
        context: context(),
    })?;
    assert_eq!(capture.observation.lifecycle.importance, 80);
    assert_eq!(
        capture.observation.lifecycle.sensitivity,
        Sensitivity::Secret
    );
    assert_eq!(capture.observation.lifecycle.confidence, 90);
    assert_eq!(capture.observation.lifecycle.freshness, 70);
    assert_eq!(
        capture.observation.lifecycle.retention,
        Retention::Temporary
    );

    let memory = store.remember_with_lifecycle(
        "A temporary note that should expire.",
        vec![],
        WorkContext::default(),
        LifecycleInput {
            retention: Some(Retention::Temporary),
            expires_at: Some((now_seconds() - 1).to_string()),
            ..LifecycleInput::default()
        },
    )?;
    assert!(store.search("temporary note", 5)?.is_empty());
    let summary = store.lifecycle_summary();
    assert_eq!(summary.temporary_observations, 1);
    assert_eq!(summary.durable_memories, 0);
    assert!(summary.sensitive_items >= 1);
    assert_eq!(summary.expired_items, 1);
    assert_eq!(memory.lifecycle.retention, Retention::Temporary);

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn older_store_records_receive_default_lifecycle_values() -> Result<(), StoreError> {
    let path = temp_store_path("legacy-lifecycle");
    let legacy = serde_json::json!({
        "version": STORE_VERSION,
        "sessions": [],
        "observations": [],
        "memories": [{
            "memory_id": "memory-legacy",
            "session_id": "session-legacy",
            "content": "legacy memory",
            "source_event_ids": [],
            "context": {},
            "created_at": "0"
        }],
        "forgotten_ids": []
    });
    fs::write(&path, serde_json::to_string(&legacy)?)?;
    let store = MemoryStore::open(&path)?;
    let results = store.search("legacy", 5)?;
    assert_eq!(results[0].lifecycle, Lifecycle::default());
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn opening_a_store_sweeps_expired_records_into_tombstones() -> Result<(), StoreError> {
    let path = temp_store_path("sweep");
    let mut store = MemoryStore::open(&path)?;
    store.remember_with_lifecycle(
        "This memory is already expired.",
        Vec::new(),
        WorkContext::default(),
        LifecycleInput {
            expires_at: Some((now_seconds() - 1).to_string()),
            retention: Some(Retention::Temporary),
            ..LifecycleInput::default()
        },
    )?;
    drop(store);

    let reopened = MemoryStore::open(&path)?;
    assert!(reopened.search("already expired", 5)?.is_empty());
    assert!(
        reopened
            .export()
            .forgotten_ids
            .iter()
            .any(|id| id.starts_with("memory-explicit-"))
    );
    fs::remove_file(path)?;
    Ok(())
}
