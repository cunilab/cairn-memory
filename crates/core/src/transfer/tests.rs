use crate::capture::ObservationInput;
use crate::lifecycle::{Lifecycle, LifecycleInput, MemoryKind, MemoryScope};
use crate::memories::Memory;
use crate::privacy::REDACTED;
use crate::recall::RecallFilter;
use crate::store::MemoryStore;
use crate::test_support::{
    HeldStoreDir, context, ids_of, import_fixture, import_forgotten, import_memories,
    import_snapshot, linked_memory, observation_for, remember_owned, temp_store_dir,
    temp_store_path,
};
use crate::transfer::{EXPORT_FORMAT, EXPORT_VERSION, ExportBundle, ImportResult};
use crate::types::{StoreError, WorkContext};
use std::collections::HashSet;
use std::fs;

#[test]
fn foreign_supersedes_link_is_stored_but_does_not_change_account_recall() -> Result<(), StoreError>
{
    let path = temp_store_path("foreign-supersedes");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let other = store.register_account("other@example.test", "correct horse battery staple")?;
    let filter = RecallFilter::default();
    let kept = remember_owned(
        &mut store,
        &owner.token,
        "alphalink checkout cache redis fits",
    )?;
    let target = remember_owned(
        &mut store,
        &owner.token,
        "alphalink checkout cache avoid redis",
    )?;
    store.start_session_for_account(&owner.token, "link-session", context())?;
    store.capture_for_account(
        &owner.token,
        observation_for("link-session", "link-evt", "resumelink resumefact"),
    )?;
    let summary = store.end_session_for_account(&owner.token, "link-session")?;
    let mut expected = vec![kept.memory_id.as_str(), target.memory_id.as_str()];
    expected.sort_unstable();

    import_memories(
        &mut store,
        &other.token,
        vec![
            linked_memory("target", &target.memory_id),
            linked_memory("summary", &summary.memory_id),
        ],
    )?;

    let hits = store.search_for_account(&owner.token, "alphalink", 10, &filter)?;
    assert_eq!(ids_of(&hits), expected);
    assert!(
        hits.iter()
            .all(|hit| hit.contradictory && hit.contradiction_ids.len() == 1)
    );
    let pack = store.context_for_account(&owner.token, "alphalink", 10, &filter)?;
    assert_eq!(ids_of(&pack.results), expected);
    let resume = store.resume_for_account(&owner.token, 10, &filter)?;
    assert_eq!(ids_of(&resume.results), vec![summary.memory_id.as_str()]);

    // The other account gets only its own rows and no content from the owner.
    let own = store.context_for_account(&other.token, "betalink", 10, &filter)?;
    assert_eq!(own.results.len(), 2);
    assert!(
        store
            .search_for_account(&other.token, "alphalink", 10, &filter)?
            .is_empty()
    );
    assert!(!format!("{own:?}").contains("alphalink"));

    // Export keeps the target row and the original link on both sides.
    let owner_export = store.export_for_account(&owner.token)?;
    assert!(
        owner_export
            .memories
            .iter()
            .any(|m| m.memory_id == target.memory_id)
    );
    let other_export = store.export_for_account(&other.token)?;
    let link = other_export
        .memories
        .iter()
        .find(|m| m.memory_id == "memory-link-target")
        .expect("stored link row");
    assert_eq!(link.supersedes.as_deref(), Some(target.memory_id.as_str()));
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn same_owner_and_unowned_successors_still_hide_their_target() -> Result<(), StoreError> {
    let path = temp_store_path("owned-supersedes");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let other = store.register_account("other@example.test", "correct horse battery staple")?;
    let filter = RecallFilter::default();

    // Same owner: the correction hides the target, and forgetting it revives the target.
    let old = remember_owned(&mut store, &owner.token, "samelink original claim")?;
    let fix = store.correct_for_account(
        &owner.token,
        &old.memory_id,
        "samelink corrected claim",
        Vec::new(),
        WorkContext::default(),
        LifecycleInput::default(),
    )?;
    let hits = store.search_for_account(&owner.token, "samelink", 10, &filter)?;
    assert_eq!(ids_of(&hits), vec![fix.memory_id.as_str()]);
    store.forget_for_account(&owner.token, &fix.memory_id)?;
    let hits = store.search_for_account(&owner.token, "samelink", 10, &filter)?;
    assert_eq!(ids_of(&hits), vec![old.memory_id.as_str()]);

    // Unowned local successor of an owned target keeps hiding it (CLI and MCP).
    let owned = remember_owned(&mut store, &owner.token, "locallink owned claim")?;
    let local_fix = store.correct(
        &owned.memory_id,
        "locallink local correction",
        Vec::new(),
        WorkContext::default(),
        LifecycleInput::default(),
    )?;
    let hits = store.search_with_filter("locallink", 10, &filter)?;
    assert_eq!(ids_of(&hits), vec![local_fix.memory_id.as_str()]);

    // An owned successor of an unowned target is foreign: the link stays inert.
    let legacy = store.remember(
        "legacylink unowned claim",
        Vec::new(),
        WorkContext::default(),
    )?;
    import_memories(
        &mut store,
        &other.token,
        vec![linked_memory("legacy", &legacy.memory_id)],
    )?;
    let hits = store.search_with_filter("legacylink", 10, &filter)?;
    assert_eq!(ids_of(&hits), vec![legacy.memory_id.as_str()]);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn foreign_link_to_unknown_target_stays_inert_after_the_target_is_restored()
-> Result<(), StoreError> {
    let path = temp_store_path("late-supersedes");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let other = store.register_account("other@example.test", "correct horse battery staple")?;
    let filter = RecallFilter::default();
    import_memories(
        &mut store,
        &other.token,
        vec![linked_memory("late", "memory-later-target")],
    )?;
    assert!(
        store
            .export_for_account(&other.token)?
            .memories
            .iter()
            .any(|m| {
                m.memory_id == "memory-link-late"
                    && m.supersedes.as_deref() == Some("memory-later-target")
            })
    );

    let mut restored = linked_memory("restored", "memory-unrelated");
    restored.memory_id = "memory-later-target".to_owned();
    restored.supersedes = None;
    restored.content = "restoredlink owner backup".to_owned();
    import_memories(&mut store, &owner.token, vec![restored])?;
    let hits = store.search_for_account(&owner.token, "restoredlink", 10, &filter)?;
    assert_eq!(ids_of(&hits), vec!["memory-later-target"]);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn foreign_tombstones_reject_the_whole_import_and_change_nothing() -> Result<(), StoreError> {
    let path = temp_store_path("foreign-tombstone");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let other = store.register_account("other@example.test", "correct horse battery staple")?;
    let filter = RecallFilter::default();
    let kept = remember_owned(&mut store, &owner.token, "tombfact alpha note")?;
    store.start_session_for_account(&owner.token, "tomb-session", context())?;
    store.capture_for_account(
        &owner.token,
        observation_for("tomb-session", "tomb-evt", "tombsummary fact"),
    )?;
    let summary = store.end_session_for_account(&owner.token, "tomb-session")?;
    let before = fs::read(&path)?;

    // A memory id, then an observation event id; each comes with a valid new row.
    for foreign in [kept.memory_id.as_str(), "tomb-evt"] {
        let error = import_forgotten(
            &mut store,
            &other.token,
            vec![linked_memory("tomb-new", "memory-unrelated")],
            &[foreign],
        )
        .expect_err("foreign tombstone must reject the import");
        assert!(
            matches!(&error, StoreError::Validation(text) if text == "account does not own the memory")
        );
        assert_eq!(fs::read(&path)?, before, "nothing is written on rejection");
    }

    let mut reopened = MemoryStore::open(&path)?;
    let hits = reopened.search_for_account(&owner.token, "tombfact", 10, &filter)?;
    assert_eq!(ids_of(&hits), vec![kept.memory_id.as_str()]);
    let resume = reopened.resume_for_account(&owner.token, 10, &filter)?;
    assert_eq!(ids_of(&resume.results), vec![summary.memory_id.as_str()]);
    assert!(
        reopened
            .export_for_account(&other.token)?
            .memories
            .is_empty()
    );
    assert!(reopened.state.forgotten_ids.is_empty());
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn unknown_tombstones_are_ignored_and_source_ids_are_not_permission() -> Result<(), StoreError> {
    let path = temp_store_path("unknown-tombstone");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let other = store.register_account("other@example.test", "correct horse battery staple")?;

    // An id nobody owns, and a source id named by the importer's own new row.
    let mut row = linked_memory("tomb-src", "memory-unrelated");
    row.source_event_ids = vec!["tomb-future-evt".to_owned()];
    let result = import_forgotten(
        &mut store,
        &other.token,
        vec![row],
        &["tomb-unknown", "tomb-future-evt", "memory-link-tomb-src"],
    )?;
    assert_eq!(result.imported_memories, 1);
    assert_eq!(
        result.imported_forgotten_ids, 1,
        "only the row's own id counts"
    );
    let expected = HashSet::from(["memory-link-tomb-src".to_owned()]);
    assert_eq!(store.state.forgotten_ids, expected);

    // The owner's later capture of that event id is not poisoned.
    store.start_session_for_account(&owner.token, "tomb-later", context())?;
    store.capture_for_account(
        &owner.token,
        observation_for("tomb-later", "tomb-future-evt", "latertomb fact"),
    )?;
    let summary = store.end_session_for_account(&owner.token, "tomb-later")?;
    assert!(summary.content.contains("latertomb"));
    assert_eq!(MemoryStore::open(&path)?.state.forgotten_ids, expected);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn own_tombstones_apply_and_forgotten_memories_survive_a_restore() -> Result<(), StoreError> {
    let path = temp_store_path("own-tombstone");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let filter = RecallFilter::default();
    let existing = remember_owned(&mut store, &owner.token, "owntomb existing note")?;
    let result = import_forgotten(
        &mut store,
        &owner.token,
        vec![linked_memory("own-new", "memory-unrelated")],
        &[&existing.memory_id, "memory-link-own-new"],
    )?;
    assert_eq!(result.imported_forgotten_ids, 2);
    assert!(
        store
            .search_for_account(&owner.token, "owntomb", 10, &filter)?
            .is_empty()
    );
    fs::remove_file(&path)?;

    // A normal export of a forgotten memory restores into a new store and account.
    let source_path = temp_store_path("restore-source");
    let mut source = MemoryStore::open(&source_path)?;
    let account = source.bootstrap_account("one@example.test", "correct horse battery staple")?;
    let keep = remember_owned(&mut source, &account.token, "restoretomb keep")?;
    let drop = remember_owned(&mut source, &account.token, "restoretomb drop")?;
    source.forget_for_account(&account.token, &drop.memory_id)?;
    let bundle = source.export_for_account(&account.token)?;
    assert_eq!(
        bundle.forgotten_ids.len(),
        2,
        "the memory and its source event id"
    );

    let target_path = temp_store_path("restore-target");
    let mut target = MemoryStore::open(&target_path)?;
    let restored = target.bootstrap_account("two@example.test", "correct horse battery staple")?;
    let result = target.import_for_account(&restored.token, bundle)?;
    assert_eq!(result.imported_memories, 2);
    assert_eq!(
        result.imported_forgotten_ids, 1,
        "the source id has no row and is ignored"
    );
    let hits = target.search_for_account(&restored.token, "restoretomb", 10, &filter)?;
    assert_eq!(ids_of(&hits), vec![keep.memory_id.as_str()]);
    fs::remove_file(source_path)?;
    fs::remove_file(target_path)?;
    Ok(())
}

// CM-AUDIT-013 B1: memory ids and event ids share one id space. A row another account
// owns under an id must make capture and sync push of that id fail loudly.
#[test]
fn a_foreign_memory_row_id_cannot_poison_a_later_capture_or_sync_push() -> Result<(), StoreError> {
    let path = temp_store_path("tombstone-namespace");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let other = store.register_account("other@example.test", "correct horse battery staple")?;
    let device = store.register_device("dev-ns", &owner.account.person_id, None, "Owner")?;
    store.start_session_for_account(&owner.token, "ns-session", context())?;

    // The importer brings a memory row named like the owner's future event id, with a tombstone.
    let mut squat = linked_memory("squat", "memory-unrelated");
    squat.memory_id = "evt-future-1".to_owned();
    squat.supersedes = None;
    let imported = import_forgotten(&mut store, &other.token, vec![squat], &["evt-future-1"])?;
    assert_eq!(imported.imported_forgotten_ids, 1);

    let events_before = store.state.sync_events.len();
    let cursor_before = store.state.next_sync_cursor;
    let input = observation_for("ns-session", "evt-future-1", "nsprivate owner fact");
    let capture = store
        .capture_for_account(&owner.token, input.clone())
        .expect_err("a foreign memory row owns the id");
    assert!(
        matches!(&capture, StoreError::Validation(text) if text == "event belongs to another person: evt-future-1")
    );
    let push = store
        .sync_push(&device.token, input.clone())
        .expect_err("the device push is rejected the same way");
    assert!(
        matches!(&push, StoreError::Validation(text) if text == "event belongs to another person: evt-future-1")
    );
    assert!(store.observations().all(|o| o.event_id != "evt-future-1"));
    assert_eq!(store.state.sync_events.len(), events_before);
    assert_eq!(store.state.next_sync_cursor, cursor_before);
    assert!(!format!("{capture}{push}").contains("nsprivate"));
    // Only the device's last-seen time may change, and memory matches the disk.
    let reopened = MemoryStore::open(&path)?;
    assert_eq!(
        reopened.state.devices[0].device.last_seen_at,
        store.state.devices[0].device.last_seen_at
    );
    assert_eq!(
        reopened.state.observation_owners.len(),
        store.state.observation_owners.len()
    );
    assert_eq!(reopened.state.sync_events.len(), events_before);
    let mut reopened = reopened;
    assert!(
        reopened
            .capture_for_account(&owner.token, input.clone())
            .is_err()
    );

    // A bad token fails on authentication, before the id is looked at.
    let bad = store
        .capture_for_account("not-a-token", input.clone())
        .expect_err("bad token");
    assert!(!bad.to_string().contains("belongs to another person"));
    let bad = store
        .sync_push("not-a-token", input)
        .expect_err("bad device token");
    assert!(!bad.to_string().contains("belongs to another person"));

    // Unaffected: a normal capture, device adoption of it, and a memory row the same
    // person owns under an id it later captures.
    store.capture_for_account(
        &owner.token,
        observation_for("ns-session", "evt-ok", "nsok fact"),
    )?;
    let adopted = store.sync_push(
        &device.token,
        observation_for("ns-session", "evt-ok", "nsok fact"),
    )?;
    assert!(adopted.accepted && !adopted.duplicate);
    let mut own = linked_memory("own", "memory-unrelated");
    own.memory_id = "evt-own-1".to_owned();
    own.supersedes = None;
    import_forgotten(&mut store, &owner.token, vec![own], &[])?;
    store.capture_for_account(
        &owner.token,
        observation_for("ns-session", "evt-own-1", "nsown fact"),
    )?;
    // An observation another person already owns cannot be claimed as a memory row.
    let mut claim = linked_memory("claim", "memory-unrelated");
    claim.memory_id = "evt-ok".to_owned();
    claim.supersedes = None;
    assert!(import_forgotten(&mut store, &other.token, vec![claim], &[]).is_err());
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn unscoped_import_still_accepts_any_tombstone() -> Result<(), StoreError> {
    let path = temp_store_path("unscoped-tombstone");
    let mut store = MemoryStore::open(&path)?;
    let mut bundle = store.export();
    bundle.forgotten_ids = vec!["unscoped-unknown".to_owned()];
    assert_eq!(store.import_bundle(bundle)?.imported_forgotten_ids, 1);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn conflicting_import_is_atomic() -> Result<(), StoreError> {
    let source_path = temp_store_path("import-conflict-source");
    let target_path = temp_store_path("import-conflict-target");
    let mut source = MemoryStore::open(&source_path)?;
    let imported = source.remember("new imported memory", Vec::new(), WorkContext::default())?;
    let mut bundle = source.export();

    let mut target = MemoryStore::open(&target_path)?;
    let existing = target.remember("existing target memory", Vec::new(), WorkContext::default())?;
    let mut conflicting = existing.clone();
    conflicting.content = "conflicting replacement".to_owned();
    bundle.memories = vec![imported.clone(), conflicting];

    let error = target
        .import_bundle(bundle)
        .expect_err("conflicting imports must fail");
    assert!(error.to_string().contains("conflicting memory"));
    assert!(
        !target
            .search("new imported memory", 5)?
            .iter()
            .any(|result| result.memory_id == imported.memory_id)
    );
    assert_eq!(target.search("existing target memory", 5)?.len(), 1);

    fs::remove_file(source_path)?;
    fs::remove_file(target_path)?;
    Ok(())
}

#[test]
fn failed_unscoped_import_persist_changes_nothing_and_retries() -> Result<(), StoreError> {
    let dir = temp_store_dir("import-commit");
    let path = dir.join("store.json");
    let mut store = MemoryStore::open(&path)?;
    store.start_session("base-session", context())?;
    let bundle = import_fixture()?;
    let before = fs::read(&path)?;
    let snapshot = import_snapshot(&store);

    let mut blocked = HeldStoreDir::block(&path);
    let failed = store
        .import_bundle(bundle.clone())
        .expect_err("a blocked store must fail the import");
    assert!(matches!(failed, StoreError::Io(_)), "got {failed}");
    assert_eq!(import_snapshot(&store), snapshot);
    blocked.release();
    assert_eq!(fs::read(&path)?, before);

    // An unrelated write must not commit the failed rows.
    store.start_session("later-session", context())?;
    assert_eq!(MemoryStore::open(&path)?.observations().count(), 0);
    assert_eq!(store.observations().count(), 0);

    let result = store.import_bundle(bundle.clone())?;
    assert_eq!(result.imported_sessions, 1);
    assert_eq!(result.imported_observations, bundle.observations.len());
    assert_eq!(result.imported_memories, bundle.memories.len());
    assert_eq!(result.imported_forgotten_ids, 1);
    let reopened = MemoryStore::open(&path)?;
    assert_eq!(import_snapshot(&reopened), import_snapshot(&store));
    assert!(reopened.state.forgotten_ids.contains("imp-expired"));
    assert!(reopened.state.forgotten_ids.contains("imp-gone"));
    assert!(!reopened.state.forgotten_ids.contains("imp-live"));
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn failed_account_import_persist_changes_nothing_and_retries() -> Result<(), StoreError> {
    let dir = temp_store_dir("account-import-commit");
    let path = dir.join("store.json");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let bundle = import_fixture()?;
    let before = fs::read(&path)?;
    let snapshot = import_snapshot(&store);

    let mut blocked = HeldStoreDir::block(&path);
    let failed = store
        .import_for_account(&owner.token, bundle.clone())
        .expect_err("a blocked store must fail the account import");
    assert!(matches!(failed, StoreError::Io(_)), "got {failed}");
    assert_eq!(import_snapshot(&store), snapshot);
    blocked.release();
    assert_eq!(fs::read(&path)?, before);

    // An unrelated write must not commit rows without their owners.
    store.start_session_for_account(&owner.token, "later-session", context())?;
    assert_eq!(MemoryStore::open(&path)?.observations().count(), 0);
    assert_eq!(store.observations().count(), 0);
    assert!(
        store
            .export_for_account(&owner.token)?
            .observations
            .is_empty()
    );

    let result = store.import_for_account(&owner.token, bundle.clone())?;
    assert_eq!(result.imported_observations, bundle.observations.len());
    assert_eq!(result.imported_memories, bundle.memories.len());
    assert_eq!(result.imported_forgotten_ids, 1);
    let reopened = MemoryStore::open(&path)?;
    assert_eq!(import_snapshot(&reopened), import_snapshot(&store));
    assert_eq!(
        reopened.state.observation_owners.len(),
        bundle.observations.len()
    );
    assert_eq!(reopened.state.memory_owners.len(), bundle.memories.len());
    assert!(reopened.state.forgotten_ids.contains("imp-expired"));
    assert!(reopened.state.forgotten_ids.contains("imp-gone"));
    assert!(reopened.state.sync_deletions.is_empty());
    let exported = reopened.export_for_account(&owner.token)?;
    assert_eq!(exported.observations.len(), bundle.observations.len());
    assert!(exported.forgotten_ids.iter().any(|id| id == "imp-expired"));
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn export_import_round_trip_is_idempotent_and_preserves_forget() -> Result<(), StoreError> {
    let source_path = temp_store_path("export-source");
    let target_path = temp_store_path("export-target");
    let bundle = {
        let mut source = MemoryStore::open(&source_path)?;
        source.start_session("sess-export", context())?;
        source.capture_observation(ObservationInput {
            event_id: "evt-export".to_owned(),
            session_id: "sess-export".to_owned(),
            content: "Exported evidence must survive a restore without replaying providers."
                .to_owned(),
            role: "assistant".to_owned(),
            source: "test".to_owned(),
            importance: Some("high".to_owned()),
            lifecycle: LifecycleInput::default(),
            context: context(),
        })?;
        source.end_session("sess-export")?;
        source.forget("evt-export")?;
        source.export()
    };

    assert_eq!(bundle.format, EXPORT_FORMAT);
    assert_eq!(bundle.version, EXPORT_VERSION);
    assert!(bundle.forgotten_ids.contains(&"evt-export".to_owned()));

    let mut target = MemoryStore::open(&target_path)?;
    let first = target.import_bundle(bundle.clone())?;
    assert_eq!(first.imported_sessions, 1);
    assert_eq!(first.imported_observations, 1);
    assert_eq!(
        first.imported_memories, 2,
        "session note and separate handoff"
    );
    assert_eq!(first.imported_forgotten_ids, 1);
    assert!(target.search("providers", 5)?.is_empty());

    let second = target.import_bundle(bundle)?;
    assert_eq!(
        second,
        ImportResult {
            imported_sessions: 0,
            imported_observations: 0,
            imported_memories: 0,
            imported_forgotten_ids: 0,
        }
    );
    drop(target);
    let mut restored = MemoryStore::open(&target_path)?;
    assert!(restored.search("providers", 5)?.is_empty());

    fs::remove_file(source_path)?;
    fs::remove_file(target_path)?;
    Ok(())
}

#[test]
fn import_rejects_unknown_version_and_redacts_payload() -> Result<(), StoreError> {
    let path = temp_store_path("export-validation");
    let mut store = MemoryStore::open(&path)?;
    let invalid = serde_json::json!({
        "format": EXPORT_FORMAT,
        "version": 99,
        "exportedAt": "0",
        "sessions": [],
        "observations": [],
        "memories": [],
        "forgottenIds": []
    });
    let error = store
        .import_json(&invalid.to_string())
        .expect_err("version must be rejected");
    assert!(error.to_string().contains("unsupported export version"));

    let bundle = ExportBundle {
        format: EXPORT_FORMAT.to_owned(),
        version: EXPORT_VERSION,
        exported_at: "0".to_owned(),
        sessions: vec![],
        observations: vec![],
        memories: vec![Memory {
            fields: Default::default(),
            memory_id: "memory-import-secret".to_owned(),
            session_id: "session-import-secret".to_owned(),
            content: "api_key=should-not-persist".to_owned(),
            source_event_ids: vec![],
            context: WorkContext::default(),
            lifecycle: Lifecycle::default(),
            kind: MemoryKind::Explicit,
            scope: MemoryScope::Global,
            project_id: None,
            next_steps: Vec::new(),
            unresolved_work: Vec::new(),
            supersedes: None,
            contradicts: Vec::new(),
            promoted_from: None,
            promotion_reason: None,
            verified_commit: None,
            published_by: None,
            changed_by: None,
            changed_at: None,
            created_at: "0".to_owned(),
            access_count: 0,
            last_accessed_at: None,
            helpful_count: 0,
            stale_count: 0,
            feedback: Vec::new(),
        }],
        forgotten_ids: vec![],
    };
    store.import_bundle(bundle)?;
    let persisted = fs::read_to_string(&path)?;
    assert!(!persisted.contains("should-not-persist"));
    assert!(persisted.contains(REDACTED));
    fs::remove_file(path)?;
    Ok(())
}
