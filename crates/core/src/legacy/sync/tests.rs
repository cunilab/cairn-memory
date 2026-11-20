use crate::legacy::accounts::WEB_ONLY_ERROR;
use crate::legacy::capture::ObservationInput;
use crate::legacy::devices::DeviceStatus;
use crate::legacy::lifecycle::LifecycleInput;
use crate::legacy::store::MemoryStore;
use crate::legacy::test_support::{
    HeldStoreDir, age_observations, empty_capture, ended, observation_for, owner_context_for_claim,
    owner_pairs, repo_context, summaries, temp_store_dir, temp_store_path,
};
use crate::legacy::types::{StoreError, WorkContext, timestamp};
use std::fs;

#[test]
fn trusted_device_sync_is_authenticated_idempotent_and_revocable() -> Result<(), StoreError> {
    let path = temp_store_path("device-sync");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let person_id = owner.account.person_id.clone();
    let first = store.register_device(
        "device-a",
        person_id.clone(),
        Some("agent-a".to_owned()),
        "MacBook",
    )?;
    let second = store.register_device("device-b", person_id, None, "Laptop")?;
    assert_eq!(first.device.status, DeviceStatus::Active);
    assert_eq!(second.device.status, DeviceStatus::Active);

    let input = observation_for(
        "sync-session",
        "sync-event-1",
        "The trusted device sync test passed.",
    );
    let pushed = store.sync_push(&first.token, input.clone())?;
    assert!(pushed.accepted);
    assert!(!pushed.duplicate);
    let duplicate = store.sync_push(&first.token, input)?;
    assert!(duplicate.duplicate);
    assert_eq!(duplicate.cursor, pushed.cursor);

    let pulled = store.sync_pull(&second.token, 0)?;
    assert_eq!(pulled.events.len(), 1);
    assert_eq!(pulled.events[0].event_id, "sync-event-1");
    let resumed = store.sync_pull(&second.token, pulled.cursor)?;
    assert!(resumed.events.is_empty());

    drop(store);
    let mut restored = MemoryStore::open(&path)?;
    let restored_pull = restored.sync_pull(&second.token, 0)?;
    assert_eq!(restored_pull.events.len(), 1);
    assert_eq!(restored_pull.events[0].event_id, "sync-event-1");

    let rejected = restored
        .rotate_device_token(&first.token, "device-a")
        .expect_err("a device token cannot mint tokens");
    assert!(rejected.to_string().contains(WEB_ONLY_ERROR));
    let rotated = restored.rotate_device_token(&owner.token, "device-a")?;
    assert_ne!(rotated.token, first.token);
    assert!(
        restored
            .sync_push(
                &first.token,
                observation_for("sync-session", "sync-event-old-token", "old token"),
            )
            .is_err()
    );

    let revoked = restored.revoke_device(&owner.token, "device-b")?;
    assert_eq!(revoked.status, DeviceStatus::Revoked);
    assert!(restored.sync_pull(&second.token, pulled.cursor).is_err());
    assert!(restored.authenticate_device(&rotated.token).is_ok());
    assert!(restored.authenticate_device(&second.token).is_err());

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn forget_reaches_a_later_pull_without_returning_the_observation() -> Result<(), StoreError> {
    let path = temp_store_path("forget-sync");
    let mut store = MemoryStore::open(&path)?;
    let account = store.bootstrap_account("forget@example.test", "correct horse battery staple")?;
    let person = account.account.person_id;
    let first = store.register_device("device-a", &person, None, "First")?;
    let second = store.register_device("device-b", &person, None, "Second")?;
    store.sync_push(
        &first.token,
        observation_for("forget-session", "forget-event", "forget me later"),
    )?;
    let pulled = store.sync_pull(&second.token, 0)?;
    assert_eq!(pulled.events.len(), 1);
    assert!(pulled.deletions.is_empty());
    store.forget("forget-event")?;
    let from_start = store.sync_pull(&second.token, 0)?;
    assert!(
        from_start
            .events
            .iter()
            .all(|event| event.event_id != "forget-event")
    );
    let next = store.sync_pull(&second.token, pulled.cursor)?;
    assert!(
        next.events
            .iter()
            .all(|event| event.event_id != "forget-event")
    );
    assert!(
        next.deletions
            .iter()
            .any(|deletion| deletion.item_id == "forget-event")
    );
    let exported = store.export();
    assert!(
        exported
            .observations
            .iter()
            .any(|observation| observation.event_id == "forget-event")
    );
    assert!(exported.forgotten_ids.iter().any(|id| id == "forget-event"));
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn sync_event_id_collision_cannot_leak_another_persons_observation() -> Result<(), StoreError> {
    let path = temp_store_path("device-sync-scope");
    let mut store = MemoryStore::open(&path)?;
    let owner_account =
        store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let other_account =
        store.register_account("other@example.test", "correct horse battery staple")?;
    let owner = store.register_device(
        "device-owner",
        &owner_account.account.person_id,
        None,
        "Owner",
    )?;
    let other = store.register_device(
        "device-other",
        &other_account.account.person_id,
        None,
        "Other",
    )?;

    store.sync_push(
        &owner.token,
        observation_for("owner-session", "shared-event", "owner private evidence"),
    )?;

    let error = store
        .sync_push(
            &other.token,
            observation_for("other-session", "shared-event", "other retry"),
        )
        .expect_err("a cross-person event ID collision must not be acknowledged");
    assert!(error.to_string().contains("another person"));

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn failed_sync_commit_does_not_keep_an_observation_without_sync_metadata() -> Result<(), StoreError>
{
    let dir = temp_store_dir("sync-capture-commit");
    let path = dir.join("store.json");
    let mut store = MemoryStore::open(&path)?;
    let owner_account =
        store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let other_account =
        store.register_account("other@example.test", "correct horse battery staple")?;
    let owner = store.register_device(
        "device-owner",
        &owner_account.account.person_id,
        None,
        "Owner",
    )?;
    let other = store.register_device(
        "device-other",
        &other_account.account.person_id,
        None,
        "Other",
    )?;
    store.sync_push(
        &owner.token,
        observation_for("sync-sess", "kept-sync", "keptalphafact"),
    )?;
    let seen_before = "baseline-seen";
    assert_ne!(seen_before, timestamp());
    store
        .state
        .devices
        .iter_mut()
        .find(|record| record.device.device_id == "device-owner")
        .expect("owner device")
        .device
        .last_seen_at = seen_before.to_owned();
    store.persist()?;
    let before = fs::read(&path)?;
    let cursor_before = store.state.next_sync_cursor;
    let events_before = store.state.sync_events.len();

    let mut blocked = HeldStoreDir::block(&path);
    let failed = store
        .sync_push(
            &owner.token,
            observation_for("sync-sess", "retry-sync", "retrybetafact"),
        )
        .expect_err("a blocked parent directory must fail the sync commit");
    assert!(
        matches!(failed, StoreError::Io(_)),
        "storage error, got {failed}"
    );
    assert_eq!(
        store
            .state
            .devices
            .iter()
            .find(|record| record.device.device_id == "device-owner")
            .expect("owner device")
            .device
            .last_seen_at,
        seen_before
    );
    assert!(
        store
            .observations()
            .all(|observation| observation.event_id != "retry-sync")
    );
    assert!(
        store
            .state
            .sync_events
            .iter()
            .all(|event| event.event_id != "retry-sync")
    );
    assert_eq!(store.state.next_sync_cursor, cursor_before);
    assert_eq!(store.state.sync_events.len(), events_before);
    assert_eq!(
        store
            .observations()
            .filter(|observation| observation.event_id == "kept-sync")
            .count(),
        1
    );
    blocked.release();
    assert_eq!(fs::read(&path)?, before);

    let retry = store.sync_push(
        &owner.token,
        observation_for("sync-sess", "retry-sync", "retrybetafact"),
    )?;
    assert!(!retry.duplicate);
    assert_eq!(retry.observation.content, "retrybetafact");
    assert_eq!(
        store
            .state
            .sync_events
            .iter()
            .filter(|event| event.event_id == "retry-sync"
                && event.person_id == owner_account.account.person_id
                && event.cursor == retry.cursor)
            .count(),
        1
    );

    drop(store);
    let mut reopened = MemoryStore::open(&path)?;
    assert_eq!(
        reopened
            .observations()
            .filter(|observation| observation.event_id == "retry-sync")
            .count(),
        1
    );
    assert_eq!(
        reopened
            .state
            .sync_events
            .iter()
            .filter(|event| event.event_id == "retry-sync"
                && event.person_id == owner_account.account.person_id)
            .count(),
        1
    );
    assert_eq!(
        reopened
            .observations()
            .filter(|observation| observation.event_id == "kept-sync")
            .count(),
        1
    );
    let pulled = reopened.sync_pull(&owner.token, 0)?;
    assert_eq!(
        pulled
            .events
            .iter()
            .filter(|event| event.event_id == "retry-sync")
            .count(),
        1
    );
    assert_eq!(
        pulled
            .events
            .iter()
            .filter(|event| event.event_id == "kept-sync")
            .count(),
        1
    );
    let collision = reopened
        .sync_push(
            &other.token,
            observation_for("other-sess", "retry-sync", "othermarkertext"),
        )
        .expect_err("another person still cannot claim the event");
    let collision_text = collision.to_string();
    assert!(collision_text.contains("event belongs to another person: retry-sync"));
    assert!(!collision_text.contains("retrybetafact"));
    assert!(!collision_text.contains("othermarkertext"));
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn owned_observation_adopts_one_sync_event() -> Result<(), StoreError> {
    let dir = temp_store_dir("sync-adopt");
    let path = dir.join("store.json");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let member = store.register_account("member@example.test", "correct horse battery staple")?;
    let owner_context = WorkContext {
        repository: Some("owner-repo".to_owned()),
        branch: Some("owner-branch".to_owned()),
        ..WorkContext::default()
    };
    let member_context = WorkContext {
        repository: Some("member-repo".to_owned()),
        branch: Some("member-branch".to_owned()),
        ..WorkContext::default()
    };
    store.start_session(
        "adopt-session",
        WorkContext {
            repository: Some("legacy-repo".to_owned()),
            branch: Some("legacy-branch".to_owned()),
            ..WorkContext::default()
        },
    )?;
    store.start_session_for_account(&owner.token, "adopt-session", owner_context)?;
    store.start_session_for_account(&member.token, "adopt-session", member_context.clone())?;
    let captured = store.capture_for_account(
        &owner.token,
        empty_capture("adopt-session", "owned-event", "canonicalalphafact"),
    )?;
    assert!(!captured.duplicate);
    assert_eq!(
        captured.observation.context.branch.as_deref(),
        Some("owner-branch")
    );
    let canonical = captured.observation.clone();
    let owner_device =
        store.register_device("device-owner", &owner.account.person_id, None, "Owner")?;
    let owner_other = store.register_device(
        "device-owner-2",
        &owner.account.person_id,
        None,
        "Owner two",
    )?;
    let member_device =
        store.register_device("device-member", &member.account.person_id, None, "Member")?;

    store.capture_observation(observation_for(
        "anon-sess",
        "anon-event",
        "anonymousmarkerfact",
    ))?;
    let owners_before_reject = owner_pairs(&store);
    let anon = store
        .sync_push(
            &owner_device.token,
            ObservationInput {
                content: owner.account.person_id.clone(),
                context: member_context.clone(),
                ..observation_for("anon-sess", "anon-event", "claimtext")
            },
        )
        .expect_err("an unowned observation is not adopted");
    let anon_text = anon.to_string();
    assert!(
        anon_text.contains("event already exists outside sync scope: anon-event"),
        "{anon_text}"
    );
    assert!(!anon_text.contains("anonymousmarkerfact"));
    assert!(!anon_text.contains("claimtext"));
    assert!(!anon_text.contains(&owner.account.person_id));
    assert_eq!(owner_pairs(&store), owners_before_reject);
    assert!(
        store
            .state
            .sync_events
            .iter()
            .all(|event| event.event_id != "anon-event")
    );

    store.capture_for_account(
        &member.token,
        observation_for("adopt-session", "member-event", "memberprivatefact"),
    )?;
    let foreign = store
        .sync_push(
            &owner_device.token,
            ObservationInput {
                content: owner.account.person_id.clone(),
                context: owner_context_for_claim(),
                session_id: "adopt-session".to_owned(),
                ..observation_for("adopt-session", "member-event", "stealtext")
            },
        )
        .expect_err("another person's observation is not acknowledged");
    let foreign_text = foreign.to_string();
    assert!(
        foreign_text.contains("event belongs to another person: member-event"),
        "{foreign_text}"
    );
    assert!(!foreign_text.contains("memberprivatefact"));
    assert!(!foreign_text.contains("stealtext"));
    assert!(!foreign_text.contains(&member.account.person_id));
    assert!(store.observation_owned_by("member-event", &member.account.person_id));
    assert!(!store.observation_owned_by("member-event", &owner.account.person_id));
    assert!(
        store
            .state
            .sync_events
            .iter()
            .all(|event| event.event_id != "member-event")
    );

    let replacement = ObservationInput {
        event_id: "owned-event".to_owned(),
        session_id: "other-session".to_owned(),
        content: "replacementbetatext".to_owned(),
        role: "user".to_owned(),
        source: "sync".to_owned(),
        importance: Some("low".to_owned()),
        lifecycle: LifecycleInput::default(),
        context: member_context,
    };
    // Changed text is not the same event: a conflict, and nothing is attached.
    let rejected = store.sync_push(&owner_device.token, replacement.clone());
    assert!(
        matches!(rejected, Err(StoreError::Conflict(_))),
        "{rejected:?}"
    );
    assert!(
        store
            .state
            .sync_events
            .iter()
            .all(|event| event.event_id != "owned-event")
    );
    // First attachment of the same event enters the sync stream, so this is not a duplicate.
    let adopted = store.sync_push(
        &owner_device.token,
        empty_capture("adopt-session", "owned-event", "canonicalalphafact"),
    )?;
    assert!(adopted.accepted);
    assert!(!adopted.duplicate);
    assert_eq!(adopted.observation, canonical);
    assert_eq!(
        store
            .observations()
            .filter(|observation| observation.event_id == "owned-event")
            .count(),
        1
    );
    assert_eq!(
        store
            .state
            .sync_events
            .iter()
            .filter(|event| event.event_id == "owned-event"
                && event.person_id == owner.account.person_id
                && event.cursor == adopted.cursor
                && event.observation == canonical)
            .count(),
        1
    );
    assert!(store.observation_owned_by("owned-event", &owner.account.person_id));
    let persisted = fs::read_to_string(&path)?;
    assert!(persisted.contains("canonicalalphafact"));
    assert!(!persisted.contains("replacementbetatext"));
    assert!(!persisted.contains("other-session"));

    // Replaying the pushed text after adoption is a changed replay of the canonical row.
    let changed = store.sync_push(&owner_device.token, replacement);
    assert!(
        matches!(changed, Err(StoreError::Conflict(_))),
        "{changed:?}"
    );
    let retry = store.sync_push(
        &owner_device.token,
        empty_capture("adopt-session", "owned-event", "canonicalalphafact"),
    )?;
    assert!(retry.duplicate);
    assert_eq!(retry.cursor, adopted.cursor);
    assert_eq!(retry.observation, canonical);
    assert_eq!(
        store
            .state
            .sync_events
            .iter()
            .filter(|event| event.event_id == "owned-event")
            .count(),
        1
    );
    assert!(!fs::read_to_string(&path)?.contains("replacementbetatext"));

    let pulled = store.sync_pull(&owner_other.token, 0)?;
    assert_eq!(
        pulled
            .events
            .iter()
            .filter(|event| event.event_id == "owned-event" && event.observation == canonical)
            .count(),
        1
    );
    let resumed = store.sync_pull(&owner_other.token, pulled.cursor)?;
    assert!(
        resumed
            .events
            .iter()
            .all(|event| event.event_id != "owned-event")
    );

    let collision = store
        .sync_push(
            &member_device.token,
            observation_for("adopt-session", "owned-event", "memberstealtext"),
        )
        .expect_err("another person's device cannot acknowledge the event");
    let collision_text = collision.to_string();
    assert!(collision_text.contains("event belongs to another person: owned-event"));
    assert!(!collision_text.contains("canonicalalphafact"));
    assert!(!collision_text.contains("memberstealtext"));

    let fresh = store.sync_push(
        &owner_device.token,
        empty_capture("adopt-session", "fresh-sync", "freshsyncfact"),
    )?;
    assert!(!fresh.duplicate);
    assert_eq!(
        fresh.observation.context.branch.as_deref(),
        Some("owner-branch")
    );
    assert_ne!(
        fresh.observation.context.repository.as_deref(),
        Some("legacy-repo")
    );
    assert_ne!(
        fresh.observation.context.branch.as_deref(),
        Some("member-branch")
    );

    drop(store);
    let mut reopened = MemoryStore::open(&path)?;
    assert_eq!(
        reopened
            .observations()
            .find(|observation| observation.event_id == "owned-event")
            .expect("canonical row")
            .content,
        "canonicalalphafact"
    );
    assert_eq!(
        reopened
            .observations()
            .filter(|observation| observation.event_id == "owned-event")
            .count(),
        1
    );
    assert!(reopened.observation_owned_by("owned-event", &owner.account.person_id));
    assert_eq!(
        reopened
            .state
            .sync_events
            .iter()
            .filter(|event| event.event_id == "owned-event"
                && event.person_id == owner.account.person_id)
            .count(),
        1
    );
    let pulled_again = reopened.sync_pull(&owner_device.token, 0)?;
    assert_eq!(
        pulled_again
            .events
            .iter()
            .filter(|event| event.event_id == "owned-event")
            .count(),
        1
    );
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn failed_sync_adoption_keeps_the_observation_and_retries() -> Result<(), StoreError> {
    let dir = temp_store_dir("sync-adopt-commit");
    let path = dir.join("store.json");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let member = store.register_account("member@example.test", "correct horse battery staple")?;
    let captured = store.capture_for_account(
        &owner.token,
        observation_for("adopt-sess", "owned-event", "canonicalalphafact"),
    )?;
    let canonical = captured.observation.clone();
    let device = store.register_device("device-owner", &owner.account.person_id, None, "Owner")?;
    let other =
        store.register_device("device-member", &member.account.person_id, None, "Member")?;
    let seen_before = "baseline-seen";
    store
        .state
        .devices
        .iter_mut()
        .find(|record| record.device.device_id == "device-owner")
        .expect("owner device")
        .device
        .last_seen_at = seen_before.to_owned();
    store.persist()?;
    let before = fs::read(&path)?;
    let owners_before = owner_pairs(&store);
    let cursor_before = store.state.next_sync_cursor;
    let events_before = store.state.sync_events.len();

    let mut blocked = HeldStoreDir::block(&path);
    let failed = store
        .sync_push(
            &device.token,
            observation_for("adopt-sess", "owned-event", "canonicalalphafact"),
        )
        .expect_err("a blocked parent directory must fail the adoption commit");
    assert!(
        matches!(failed, StoreError::Io(_)),
        "storage error, got {failed}"
    );
    assert_eq!(
        store
            .state
            .devices
            .iter()
            .find(|record| record.device.device_id == "device-owner")
            .expect("owner device")
            .device
            .last_seen_at,
        seen_before
    );
    assert_eq!(store.state.next_sync_cursor, cursor_before);
    assert_eq!(store.state.sync_events.len(), events_before);
    assert!(
        store
            .state
            .sync_events
            .iter()
            .all(|event| event.event_id != "owned-event")
    );
    assert_eq!(
        store
            .observations()
            .find(|observation| observation.event_id == "owned-event")
            .cloned(),
        Some(canonical.clone())
    );
    assert_eq!(owner_pairs(&store), owners_before);
    assert!(store.observation_owned_by("owned-event", &owner.account.person_id));
    blocked.release();
    assert_eq!(fs::read(&path)?, before);

    let retry = store.sync_push(
        &device.token,
        observation_for("adopt-sess", "owned-event", "canonicalalphafact"),
    )?;
    assert!(!retry.duplicate);
    assert_eq!(retry.observation, canonical);
    assert_eq!(
        store
            .state
            .sync_events
            .iter()
            .filter(|event| event.event_id == "owned-event"
                && event.cursor == retry.cursor
                && event.person_id == owner.account.person_id)
            .count(),
        1
    );
    let changed = store.sync_push(
        &device.token,
        observation_for("other-sess", "owned-event", "latermarkertext"),
    );
    assert!(
        matches!(changed, Err(StoreError::Conflict(_))),
        "{changed:?}"
    );
    let again = store.sync_push(
        &device.token,
        empty_capture("adopt-sess", "owned-event", "canonicalalphafact"),
    )?;
    assert!(again.duplicate);
    assert_eq!(again.cursor, retry.cursor);
    assert_eq!(again.observation, canonical);

    drop(store);
    let mut reopened = MemoryStore::open(&path)?;
    assert_eq!(
        reopened
            .observations()
            .find(|observation| observation.event_id == "owned-event")
            .expect("canonical row")
            .content,
        "canonicalalphafact"
    );
    assert!(reopened.observation_owned_by("owned-event", &owner.account.person_id));
    assert_eq!(
        reopened
            .state
            .sync_events
            .iter()
            .filter(|event| event.event_id == "owned-event")
            .count(),
        1
    );
    let pulled = reopened.sync_pull(&device.token, 0)?;
    assert_eq!(
        pulled
            .events
            .iter()
            .filter(|event| event.event_id == "owned-event"
                && event.observation.content == "canonicalalphafact")
            .count(),
        1
    );
    let collision = reopened
        .sync_push(
            &other.token,
            observation_for("adopt-sess", "owned-event", "memberstealtext"),
        )
        .expect_err("a foreign device still cannot acknowledge the event");
    let collision_text = collision.to_string();
    assert!(collision_text.contains("event belongs to another person: owned-event"));
    assert!(!collision_text.contains("canonicalalphafact"));
    let _ = fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn file_sync_push_attaches_initialized_project_and_closes_idle_sessions() -> Result<(), StoreError>
{
    let path = temp_store_path("sync-housekeeping");
    let mut store = MemoryStore::open(&path)?;
    let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
    let a_id = a.account.person_id.clone();
    let device = store.register_device("dev-house", &a_id, None, "Hook")?;
    store.start_session_for_account(&device.token, "idle-push", WorkContext::default())?;
    let mut first = observation_for("idle-push", "evt-idle-push", "pushidle");
    first.context = repo_context("push-repo", Some("feat/push"));
    let initialized = store.register_project(&a.token, "push-repo", "Push", None)?;
    first.context.project_id = Some(initialized.project_id);
    store.sync_push(&device.token, first)?;
    let projects = store.projects(&a.token)?;
    assert_eq!(projects.len(), 1, "push registers the project");
    assert_eq!(projects[0].repository, "push-repo");
    let branches = store.branches(&a.token, &projects[0].project_id)?;
    assert_eq!(branches.len(), 1);
    assert_eq!(branches[0].name, "feat/push");
    age_observations(&mut store, "idle-push", 31 * 60);
    assert!(!ended(&store, &a_id, "idle-push"));

    store.sync_push(
        &device.token,
        observation_for("trigger", "evt-trigger", "trigger"),
    )?;

    assert!(
        ended(&store, &a_id, "idle-push"),
        "push sweeps idle sessions"
    );
    assert_eq!(summaries(&store, "idle-push").len(), 1);
    fs::remove_file(path)?;
    Ok(())
}
