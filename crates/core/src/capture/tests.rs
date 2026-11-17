use crate::capture::{MAX_OBSERVATION_BYTES, ObservationInput};
use crate::lifecycle::{LifecycleInput, MemoryScope};
use crate::recall::RecallFilter;
use crate::store::MemoryStore;
use crate::test_support::{
    HeldStoreDir, context, empty_capture, observation, observation_for, temp_store_dir,
    temp_store_path,
};
use crate::types::{StoreError, WorkContext};
use serde_json::Value;
use std::fs;

#[test]
fn account_sessions_isolate_context_for_one_public_id() -> Result<(), StoreError> {
    let path = temp_store_path("session-person");
    fs::write(
        &path,
        r#"{"version":1,"sessions":[{"session_id":"shared-session","context":{"repository":"legacy-repo","branch":"legacy-branch","worktree":"legacy-tree","commit":null},"started_at":"1","ended_at":null}]}"#,
    )?;
    let mut store = MemoryStore::open(&path)?;
    let legacy = store
        .sessions()
        .find(|session| session.session_id == "shared-session")
        .cloned()
        .expect("legacy session");
    assert_eq!(legacy.person_id, None);
    assert_eq!(legacy.context.branch.as_deref(), Some("legacy-branch"));
    assert!(
        serde_json::to_value(&legacy)
            .expect("session json")
            .get("person_id")
            .is_none()
    );

    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let member = store.register_account("member@example.test", "correct horse battery staple")?;
    let owner_context = WorkContext {
        repository: Some("owner-repo".to_owned()),
        branch: Some("owner-branch".to_owned()),
        worktree: Some("owner-tree".to_owned()),
        ..WorkContext::default()
    };
    let member_context = WorkContext {
        repository: Some("member-repo".to_owned()),
        branch: Some("member-branch".to_owned()),
        ..WorkContext::default()
    };
    let session_id = "shared-session";
    let started =
        store.start_session_for_account(&owner.token, session_id, owner_context.clone())?;
    assert_eq!(started.context, owner_context);
    assert_eq!(
        started.person_id.as_deref(),
        Some(owner.account.person_id.as_str())
    );
    let retry =
        store.start_session_for_account(&owner.token, session_id, member_context.clone())?;
    assert_eq!(retry.context, owner_context);
    assert!(retry.ended_at.is_none());
    let member_started =
        store.start_session_for_account(&member.token, session_id, member_context.clone())?;
    assert_eq!(member_started.context, member_context);
    let unscoped = store.start_session(session_id, owner_context.clone())?;
    assert_eq!(unscoped.person_id, None);
    assert_eq!(unscoped.context.branch.as_deref(), Some("legacy-branch"));
    assert_eq!(
        store
            .sessions()
            .filter(|session| session.session_id == session_id)
            .count(),
        3
    );

    let owner_obs = store.capture_for_account(
        &owner.token,
        empty_capture(session_id, "owner-evt", "alphaquux ownerfact"),
    )?;
    assert_eq!(
        owner_obs.observation.context.branch.as_deref(),
        Some("owner-branch")
    );
    assert_eq!(
        owner_obs.observation.context.worktree.as_deref(),
        Some("owner-tree")
    );
    let sent = WorkContext {
        files: vec!["member-file.rs".to_owned()],
        ..WorkContext::default()
    };
    let member_sent = store.capture_for_account(
        &member.token,
        ObservationInput {
            context: sent.clone(),
            ..observation_for(session_id, "member-evt", "betazet memberfact")
        },
    )?;
    assert_eq!(member_sent.observation.context, sent);
    let member_empty = store.capture_for_account(
        &member.token,
        empty_capture(session_id, "member-empty", "memberemptyfact"),
    )?;
    assert_eq!(
        member_empty.observation.context.branch.as_deref(),
        Some("member-branch")
    );
    assert!(member_empty.observation.context.worktree.is_none());
    let legacy_obs =
        store.capture_observation(empty_capture(session_id, "legacy-evt", "legacyfact"))?;
    assert_eq!(
        legacy_obs.observation.context.branch.as_deref(),
        Some("legacy-branch")
    );
    let no_session = store.capture_for_account(
        &owner.token,
        empty_capture("never-started", "no-session-evt", "nosessionfact"),
    )?;
    assert_eq!(no_session.observation.context, WorkContext::default());
    assert!(
        store
            .sessions()
            .all(|session| session.session_id != "never-started")
    );

    store.start_session(
        "unscoped-only",
        WorkContext {
            branch: Some("only-unscoped".to_owned()),
            ..WorkContext::default()
        },
    )?;
    let missing = store
        .end_session_for_account(&owner.token, "unscoped-only")
        .expect_err("an account does not close the unscoped row");
    assert!(
        missing
            .to_string()
            .contains("unknown session: unscoped-only")
    );
    assert!(
        store
            .sessions()
            .find(|session| session.session_id == "unscoped-only")
            .expect("unscoped row")
            .ended_at
            .is_none()
    );
    store.start_session_for_account(&owner.token, "account-only", owner_context.clone())?;
    let missing = store
        .end_session("account-only")
        .expect_err("an unscoped end does not close an account row");
    assert!(
        missing
            .to_string()
            .contains("unknown session: account-only")
    );
    let missing = store
        .end_session_for_account(&member.token, "account-only")
        .expect_err("another account does not close this row");
    assert!(
        missing
            .to_string()
            .contains("unknown session: account-only")
    );
    assert!(
        store
            .sessions()
            .find(|session| session.session_id == "account-only")
            .expect("account row")
            .ended_at
            .is_none()
    );

    let owner_summary = store.end_session_for_account(&owner.token, session_id)?;
    assert_eq!(owner_summary.scope, MemoryScope::Personal);
    assert_eq!(
        owner_summary.context.branch.as_deref(),
        Some("owner-branch")
    );
    assert!(owner_summary.content.contains("ownerfact"));
    assert!(!owner_summary.content.contains("memberfact"));
    assert!(!owner_summary.content.contains("legacyfact"));
    assert!(
        store
            .sessions()
            .find(|session| {
                session.session_id == session_id && session.person_id == member_started.person_id
            })
            .expect("member row")
            .ended_at
            .is_none()
    );
    let resumed =
        store.start_session_for_account(&owner.token, session_id, member_context.clone())?;
    assert_eq!(resumed.context, owner_context);
    assert!(resumed.ended_at.is_some());
    let member_summary = store.end_session_for_account(&member.token, session_id)?;
    assert_eq!(member_summary.scope, MemoryScope::Personal);
    assert_eq!(
        member_summary.context.repository.as_deref(),
        Some("member-repo")
    );
    assert!(member_summary.content.contains("memberfact"));
    assert!(!member_summary.content.contains("ownerfact"));
    assert!(
        store
            .sessions()
            .find(|session| session.session_id == session_id && session.person_id.is_none())
            .expect("legacy row")
            .ended_at
            .is_none()
    );
    let legacy_summary = store.end_session(session_id)?;
    assert_eq!(legacy_summary.memory_id, format!("memory-{session_id}"));
    assert_eq!(legacy_summary.scope, MemoryScope::Personal);
    assert!(legacy_summary.content.contains("legacyfact"));
    assert!(!legacy_summary.content.contains("ownerfact"));

    let owner_hits =
        store.search_for_account(&owner.token, "ownerfact", 5, &RecallFilter::default())?;
    assert_eq!(owner_hits.len(), 2, "session note and linked handoff");
    assert_eq!(
        owner_hits[0].context.branch.as_deref(),
        Some("owner-branch")
    );
    assert!(
        store
            .search_for_account(&member.token, "ownerfact", 5, &RecallFilter::default())?
            .is_empty()
    );

    let owner_export = store.export_for_account(&owner.token)?;
    assert!(
        owner_export.sessions.iter().all(|session| {
            session.person_id.as_deref() == Some(owner.account.person_id.as_str())
        })
    );
    assert!(
        owner_export
            .sessions
            .iter()
            .any(|session| session.context == owner_context)
    );
    assert!(
        owner_export
            .sessions
            .iter()
            .all(
                |session| session.context.branch.as_deref() != Some("member-branch")
                    && session.context.branch.as_deref() != Some("legacy-branch")
            )
    );
    assert!(
        owner_export
            .observations
            .iter()
            .all(|observation| observation.event_id != "member-evt"
                && observation.event_id != "legacy-evt")
    );
    let member_export = store.export_for_account(&member.token)?;
    assert!(member_export.sessions.iter().all(|session| {
        session.person_id.as_deref() == Some(member.account.person_id.as_str())
            && session.context.repository.as_deref() != Some("owner-repo")
    }));
    let unscoped_export = store.export();
    assert!(
        unscoped_export
            .sessions
            .iter()
            .all(|session| session.person_id.is_none())
    );
    assert!(
        unscoped_export
            .sessions
            .iter()
            .any(|session| session.context.branch.as_deref() == Some("legacy-branch"))
    );
    assert_eq!(
        store
            .import_for_account(&owner.token, owner_export.clone())?
            .imported_sessions,
        0
    );
    let mut foreign_sessions = owner_export.clone();
    foreign_sessions.observations.clear();
    foreign_sessions.memories.clear();
    foreign_sessions.forgotten_ids.clear();
    let session_count = store.sessions().count();
    let rejected = store
        .import_for_account(&member.token, foreign_sessions)
        .expect_err("a foreign person_id is not an authoritative session");
    assert!(
        rejected
            .to_string()
            .contains("account does not own the memory")
    );
    assert_eq!(store.sessions().count(), session_count);

    let mut old_bundle = owner_export.clone();
    for session in &mut old_bundle.sessions {
        session.person_id = None;
    }
    assert_eq!(
        store
            .import_for_account(&owner.token, old_bundle)?
            .imported_sessions,
        0
    );
    assert_eq!(
        store
            .sessions()
            .filter(|session| session.session_id == session_id && session.person_id.is_none())
            .count(),
        1
    );

    let restored_path = temp_store_path("session-person-import");
    let mut restored = MemoryStore::open(&restored_path)?;
    let foreign_bundle = owner_export.clone();
    restored.import_bundle(owner_export)?;
    restored.import_bundle(member_export)?;
    assert!(restored.sessions().any(|session| {
        session.person_id.as_deref() == Some(owner.account.person_id.as_str())
            && session.context == owner_context
    }));
    assert!(restored.sessions().any(|session| {
        session.person_id.as_deref() == Some(member.account.person_id.as_str())
            && session.context == member_context
    }));
    let fresh_path = temp_store_path("session-person-foreign");
    let mut fresh = MemoryStore::open(&fresh_path)?;
    let stranger =
        fresh.bootstrap_account("stranger@example.test", "correct horse battery staple")?;
    let rejected = fresh
        .import_for_account(&stranger.token, foreign_bundle)
        .expect_err("a foreign person_id is not an authoritative session");
    assert!(
        rejected
            .to_string()
            .contains("account does not own the memory")
    );
    assert_eq!(fresh.sessions().count(), 0);
    let bound = fresh.import_for_account(&stranger.token, unscoped_export)?;
    assert!(bound.imported_sessions >= 1);
    assert!(fresh.sessions().all(|session| {
        session.person_id.as_deref() == Some(stranger.account.person_id.as_str())
    }));
    assert!(
        fresh
            .sessions()
            .any(|session| session.context.branch.as_deref() == Some("legacy-branch"))
    );

    let file: Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
    assert_eq!(file["version"], 1);
    drop(store);
    let reopened = MemoryStore::open(&path)?;
    assert_eq!(
        reopened
            .sessions()
            .filter(|session| session.session_id == session_id)
            .count(),
        3
    );
    assert!(reopened.sessions().any(|session| {
        session.person_id.as_deref() == Some(owner.account.person_id.as_str())
            && session.context == owner_context
            && session.ended_at.is_some()
    }));
    assert!(reopened.sessions().any(|session| {
        session.person_id.as_deref() == Some(member.account.person_id.as_str())
            && session.context == member_context
            && session.ended_at.is_some()
    }));
    assert!(reopened.sessions().any(|session| {
        session.person_id.is_none()
            && session.context.branch.as_deref() == Some("legacy-branch")
            && session.ended_at.is_some()
    }));

    fs::remove_file(path)?;
    fs::remove_file(restored_path)?;
    fs::remove_file(fresh_path)?;
    Ok(())
}

#[test]
fn two_accounts_keep_distinct_session_summaries_for_one_session_id() -> Result<(), StoreError> {
    let path = temp_store_path("shared-session");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let member = store.register_account("member@example.test", "correct horse battery staple")?;
    let session_id = "shared-session";
    store.start_session_for_account(&owner.token, session_id, WorkContext::default())?;
    store.start_session_for_account(&member.token, session_id, WorkContext::default())?;
    store.capture_for_account(
        &owner.token,
        observation_for(session_id, "owner-evt", "alphaquux ownerfact"),
    )?;
    store.capture_for_account(
        &member.token,
        observation_for(session_id, "member-evt", "betazet memberfact"),
    )?;
    let owner_summary = store.end_session_for_account(&owner.token, session_id)?;
    let member_summary = store.end_session_for_account(&member.token, session_id)?;
    assert_ne!(owner_summary.memory_id, member_summary.memory_id);
    assert_eq!(
        owner_summary.memory_id,
        format!("memory-{}-{session_id}", owner.account.person_id)
    );
    assert_eq!(
        member_summary.memory_id,
        format!("memory-{}-{session_id}", member.account.person_id)
    );
    assert!(owner_summary.content.contains("ownerfact"));
    assert!(!owner_summary.content.contains("memberfact"));
    let owner_hits =
        store.search_for_account(&owner.token, "ownerfact", 5, &RecallFilter::default())?;
    assert_eq!(owner_hits.len(), 2, "session note and linked handoff");
    assert!(
        owner_hits
            .iter()
            .any(|m| m.memory_id == owner_summary.memory_id)
    );
    assert!(
        owner_hits
            .iter()
            .all(|m| m.source_event_ids == owner_summary.source_event_ids)
    );
    let member_hits =
        store.search_for_account(&member.token, "ownerfact", 5, &RecallFilter::default())?;
    assert!(member_hits.is_empty());
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn capture_for_account_duplicate_stays_with_the_owner() -> Result<(), StoreError> {
    let path = temp_store_path("account-capture-scope");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let member = store.register_account("member@example.test", "correct horse battery staple")?;

    let created = store.capture_for_account(
        &owner.token,
        observation_for("sess-evt", "shared-event", "owner private marker"),
    )?;
    assert!(!created.duplicate);
    assert_eq!(created.observation.content, "owner private marker");

    let retry = store.capture_for_account(
        &owner.token,
        observation_for("sess-evt", "shared-event", "owner private marker"),
    )?;
    assert!(retry.duplicate);
    assert_eq!(retry.observation.content, "owner private marker");
    assert_eq!(store.observations().count(), 1);
    assert!(store.observation_owned_by("shared-event", &owner.account.person_id));
    assert!(!store.observation_owned_by("shared-event", &member.account.person_id));

    let foreign = store
        .capture_for_account(
            &member.token,
            observation_for("sess-evt", "shared-event", "member marker"),
        )
        .expect_err("a foreign event id must not return the stored observation");
    let foreign_text = foreign.to_string();
    assert!(
        foreign_text.contains("event belongs to another person: shared-event"),
        "{foreign_text}"
    );
    assert!(!foreign_text.contains("owner private marker"));
    assert!(!foreign_text.contains("member marker"));
    assert!(!foreign_text.contains(&owner.account.person_id));
    assert!(!foreign_text.contains(&member.account.person_id));
    assert_eq!(store.observations().count(), 1);
    assert_eq!(
        store
            .observations()
            .find(|observation| observation.event_id == "shared-event")
            .expect("owned row remains")
            .content,
        "owner private marker"
    );
    assert!(store.observation_owned_by("shared-event", &owner.account.person_id));
    assert!(!store.observation_owned_by("shared-event", &member.account.person_id));
    assert_eq!(
        store
            .state
            .observation_owners
            .iter()
            .filter(|record| record.event_id == "shared-event")
            .count(),
        1
    );

    store.capture_observation(observation_for(
        "sess-anon",
        "anon-event",
        "anonymous marker",
    ))?;
    let owners_before_claim = store.state.observation_owners.len();
    let unowned = store
        .capture_for_account(
            &owner.token,
            observation_for("sess-anon", "anon-event", "claim marker"),
        )
        .expect_err("an unowned event must not be claimed or returned");
    let unowned_text = unowned.to_string();
    assert!(
        unowned_text.contains("event already exists outside sync scope: anon-event"),
        "{unowned_text}"
    );
    assert!(!unowned_text.contains("anonymous marker"));
    assert!(!unowned_text.contains("claim marker"));
    assert!(!unowned_text.contains(&owner.account.person_id));
    assert_eq!(store.observations().count(), 2);
    assert_eq!(
        store
            .observations()
            .find(|observation| observation.event_id == "anon-event")
            .expect("anonymous row remains")
            .content,
        "anonymous marker"
    );
    assert!(!store.observation_owned_by("anon-event", &owner.account.person_id));
    assert!(!store.observation_owned_by("anon-event", &member.account.person_id));
    assert_eq!(store.state.observation_owners.len(), owners_before_claim);
    assert!(
        !store
            .state
            .observation_owners
            .iter()
            .any(|record| record.event_id == "anon-event")
    );

    let unauthenticated = store
        .capture_for_account(
            "not-a-session-token",
            observation_for("sess-evt", "shared-event", "owner private marker"),
        )
        .expect_err("authentication is required before ownership is examined");
    let unauthenticated_text = unauthenticated.to_string();
    assert!(unauthenticated_text.contains("account authentication failed"));
    assert!(!unauthenticated_text.contains("owner private marker"));
    assert!(!unauthenticated_text.contains("event belongs to another person"));
    assert_eq!(store.observations().count(), 2);
    assert_eq!(store.state.observation_owners.len(), owners_before_claim);

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn a_replay_with_changed_content_or_outside_sync_is_a_conflict() -> Result<(), StoreError> {
    let path = temp_store_path("changed-replay");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let device =
        store.register_device("device-a", owner.account.person_id.clone(), None, "MacBook")?;
    let original = observation_for("sess-a", "evt-a", "original content");
    store.capture_for_account(&owner.token, original.clone())?;
    let pushed = store.sync_push(&device.token, original.clone())?;
    let persisted = fs::read(&path)?;

    let changes = [
        ObservationInput {
            content: "changed content".to_owned(),
            ..original.clone()
        },
        ObservationInput {
            session_id: "sess-b".to_owned(),
            ..original.clone()
        },
        ObservationInput {
            role: "user".to_owned(),
            ..original.clone()
        },
        ObservationInput {
            source: "other".to_owned(),
            ..original.clone()
        },
    ];
    for changed in changes {
        let account = store.capture_for_account(&owner.token, changed.clone());
        let device_push = store.sync_push(&device.token, changed);
        assert!(
            matches!(account, Err(StoreError::Conflict(_))),
            "{account:?}"
        );
        assert!(
            matches!(device_push, Err(StoreError::Conflict(_))),
            "{device_push:?}"
        );
    }
    assert_eq!(fs::read(&path)?, persisted, "a conflict writes nothing");

    // An identical replay is still acknowledged.
    assert!(
        store
            .capture_for_account(&owner.token, original.clone())?
            .duplicate
    );
    let replay = store.sync_push(&device.token, original)?;
    assert!(replay.duplicate && replay.cursor == pushed.cursor);

    // An unowned row (an unscoped capture) is outside sync scope: a conflict, not a 400.
    store.capture_observation(observation("evt-unowned", "unowned content"))?;
    let outside = store.sync_push(
        &device.token,
        observation_for("sess-a", "evt-unowned", "unowned content"),
    );
    assert!(
        matches!(&outside, Err(StoreError::Conflict(message)) if message.contains("outside sync scope")),
        "{outside:?}"
    );
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn adoption_of_an_account_capture_requires_the_same_content() -> Result<(), StoreError> {
    let path = temp_store_path("adopt-parity");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let device =
        store.register_device("device-a", owner.account.person_id.clone(), None, "MacBook")?;
    let original = observation_for("sess-a", "evt-a", "original content");
    let captured = store.capture_for_account(&owner.token, original.clone())?;
    let persisted = fs::read(&path)?;

    // Any changed field is a conflict on the first push too, and nothing is written.
    for changed in [
        ObservationInput {
            content: "changed content".to_owned(),
            ..original.clone()
        },
        ObservationInput {
            session_id: "sess-b".to_owned(),
            ..original.clone()
        },
        ObservationInput {
            role: "user".to_owned(),
            ..original.clone()
        },
        ObservationInput {
            source: "other".to_owned(),
            ..original.clone()
        },
    ] {
        let result = store.sync_push(&device.token, changed);
        assert!(matches!(result, Err(StoreError::Conflict(_))), "{result:?}");
    }
    assert_eq!(fs::read(&path)?, persisted, "a conflict writes nothing");

    // A matching push adopts the stored row once; a replay is then a duplicate.
    let adopted = store.sync_push(&device.token, original.clone())?;
    assert!(adopted.accepted && !adopted.duplicate);
    assert_eq!(adopted.observation, captured.observation);
    let replay = store.sync_push(&device.token, original)?;
    assert!(replay.duplicate && replay.cursor == adopted.cursor);
    assert_eq!(store.observations().count(), 1);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn duplicate_event_is_acknowledged_without_duplication() -> Result<(), StoreError> {
    let path = temp_store_path("deduplication");
    let mut store = MemoryStore::open(&path)?;
    store.start_session("sess-auth-001", context())?;
    let first = store.capture_observation(observation("evt-duplicate", "same event"))?;
    let second = store.capture_observation(observation("evt-duplicate", "same event"))?;

    assert!(!first.duplicate);
    assert!(second.duplicate);
    assert_eq!(store.observations().count(), 1);
    assert_eq!(second.observation.content, "same event");
    let persisted = fs::read(&path)?;
    let repeated = store.capture_observation(observation("evt-duplicate", "same event"))?;
    assert!(repeated.duplicate);
    assert_eq!(fs::read(&path)?, persisted);

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn failed_capture_persist_retries_instead_of_acknowledging_a_duplicate() -> Result<(), StoreError> {
    let dir = temp_store_dir("capture-commit");
    let path = dir.join("store.json");
    let mut store = MemoryStore::open(&path)?;
    store.capture_observation(observation("kept-event", "keptalphafact"))?;
    let before = fs::read(&path)?;

    let mut blocked = HeldStoreDir::block(&path);
    let failed = store
        .capture_observation(observation("retry-event", "retrybetafact"))
        .expect_err("a blocked store rename must fail the capture");
    assert!(
        matches!(failed, StoreError::Io(_)),
        "storage error, got {failed}"
    );
    assert!(
        store
            .observations()
            .all(|observation| observation.event_id != "retry-event")
    );
    assert_eq!(
        store
            .observations()
            .filter(|observation| observation.event_id == "kept-event")
            .count(),
        1
    );
    blocked.release();
    assert_eq!(fs::read(&path)?, before);

    let retry = store.capture_observation(observation("retry-event", "retrybetafact"))?;
    assert!(!retry.duplicate);
    assert_eq!(retry.observation.content, "retrybetafact");
    let persisted = fs::read(&path)?;
    let duplicate = store.capture_observation(observation("retry-event", "retrybetafact"))?;
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.observation.content, "retrybetafact");
    assert_eq!(fs::read(&path)?, persisted);

    drop(store);
    let reopened = MemoryStore::open(&path)?;
    assert_eq!(
        reopened
            .observations()
            .filter(|observation| observation.event_id == "retry-event")
            .count(),
        1
    );
    assert_eq!(
        reopened
            .observations()
            .filter(|observation| observation.event_id == "kept-event")
            .count(),
        1
    );
    assert_eq!(reopened.observations().count(), 2);
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn failed_account_capture_persist_leaves_no_unowned_observation() -> Result<(), StoreError> {
    let dir = temp_store_dir("account-capture-commit");
    let path = dir.join("store.json");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let member = store.register_account("member@example.test", "correct horse battery staple")?;
    store.start_session_for_account(&owner.token, "sess-commit", WorkContext::default())?;
    store.capture_for_account(
        &owner.token,
        observation_for("sess-commit", "kept-event", "keptalphafact"),
    )?;
    let before = fs::read(&path)?;
    let owners_before = store.state.observation_owners.len();

    let mut blocked = HeldStoreDir::block(&path);
    let failed = store
        .capture_for_account(
            &owner.token,
            observation_for("sess-commit", "retry-event", "retrybetafact"),
        )
        .expect_err("a blocked store rename must fail the account capture");
    assert!(
        matches!(failed, StoreError::Io(_)),
        "storage error, got {failed}"
    );
    assert!(
        store
            .observations()
            .all(|observation| observation.event_id != "retry-event")
    );
    assert!(!store.observation_owned_by("retry-event", &owner.account.person_id));
    assert_eq!(store.state.observation_owners.len(), owners_before);
    assert_eq!(
        store
            .observations()
            .filter(|observation| observation.event_id == "kept-event")
            .count(),
        1
    );
    blocked.release();
    assert_eq!(fs::read(&path)?, before);

    let retry = store.capture_for_account(
        &owner.token,
        observation_for("sess-commit", "retry-event", "retrybetafact"),
    )?;
    assert!(!retry.duplicate);
    assert_eq!(retry.observation.content, "retrybetafact");
    assert!(store.observation_owned_by("retry-event", &owner.account.person_id));
    assert!(!store.observation_owned_by("retry-event", &member.account.person_id));
    let persisted = fs::read(&path)?;
    let duplicate = store.capture_for_account(
        &owner.token,
        observation_for("sess-commit", "retry-event", "retrybetafact"),
    )?;
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.observation.content, "retrybetafact");
    assert_eq!(fs::read(&path)?, persisted);

    let foreign = store
        .capture_for_account(
            &member.token,
            observation_for("sess-commit", "retry-event", "member marker"),
        )
        .expect_err("foreign event stays rejected");
    let foreign_text = foreign.to_string();
    assert!(foreign_text.contains("event belongs to another person: retry-event"));
    assert!(!foreign_text.contains("retrybetafact"));
    assert_eq!(fs::read(&path)?, persisted);

    store.end_session_for_account(&owner.token, "sess-commit")?;
    drop(store);
    let mut reopened = MemoryStore::open(&path)?;
    assert_eq!(
        reopened
            .observations()
            .filter(|observation| observation.event_id == "retry-event")
            .count(),
        1
    );
    assert!(reopened.observation_owned_by("retry-event", &owner.account.person_id));
    assert!(!reopened.observation_owned_by("retry-event", &member.account.person_id));
    let hits =
        reopened.search_for_account(&owner.token, "retrybetafact", 5, &RecallFilter::default())?;
    assert_eq!(hits.len(), 2, "session note and linked handoff");
    assert!(hits.iter().any(|m| m.content.contains("retrybetafact")));
    assert!(
        hits.iter()
            .all(|m| m.source_event_ids.contains(&"retry-event".to_owned()))
    );
    let member_hits =
        reopened.search_for_account(&member.token, "retrybetafact", 5, &RecallFilter::default())?;
    assert!(member_hits.is_empty());
    assert_eq!(
        reopened
            .observations()
            .filter(|observation| observation.event_id == "kept-event")
            .count(),
        1
    );
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn capture_inherits_context_from_started_session() -> Result<(), StoreError> {
    let path = temp_store_path("session-context");
    let mut store = MemoryStore::open(&path)?;
    store.start_session("sess-context", context())?;
    let captured = store.capture_observation(ObservationInput {
        event_id: "evt-context".to_owned(),
        session_id: "sess-context".to_owned(),
        content: "The session context should follow this capture.".to_owned(),
        role: "agent".to_owned(),
        source: "cli".to_owned(),
        importance: None,
        lifecycle: LifecycleInput::default(),
        context: WorkContext::default(),
    })?;

    assert_eq!(captured.observation.context, context());
    let memory = store.end_session("sess-context")?;
    assert_eq!(memory.scope, MemoryScope::Personal);

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn rejects_unbounded_observations() {
    let input = observation("evt-too-large", &"x".repeat(MAX_OBSERVATION_BYTES + 1));
    let path = temp_store_path("bounded-input");
    let mut store = MemoryStore::open(&path).expect("store opens");
    let error = store
        .capture_observation(input)
        .expect_err("oversized observation must be rejected");
    assert!(error.to_string().contains("byte limit"));
    let _ = fs::remove_file(path);
}
