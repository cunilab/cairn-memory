use crate::accounts::{AccountStatus, WEB_ONLY_ERROR, hash_token};
use crate::devices::DEVICE_TOKEN_PREFIX;
use crate::lifecycle::{LifecycleInput, MemoryKind};
use crate::store::MemoryStore;
use crate::test_support::{
    context, join_project, observation_for, owned_project, remember_with_context, temp_store_path,
};
use crate::types::StoreError;
use std::fs;

#[test]
fn device_token_is_an_agent_credential_scoped_to_its_account() -> Result<(), StoreError> {
    let path = temp_store_path("device-agent");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let other = store.register_account("other@example.test", "correct horse battery staple")?;
    let device = store.register_device(
        "device-own",
        owner.account.person_id.clone(),
        None,
        "Laptop",
    )?;
    let other_device = store.register_device(
        "device-other",
        other.account.person_id.clone(),
        None,
        "Other",
    )?;

    // Agent actions run as the owning account and stay inside it.
    let (project_id, _) = owned_project(&mut store, &device.token, "device made decision")?;
    assert_eq!(
        store.authenticate_account(&device.token)?.person_id,
        owner.account.person_id
    );
    assert_eq!(store.project_memories(&owner.token, &project_id)?.len(), 1);
    let foreign = store
        .project_memories(&other_device.token, &project_id)
        .expect_err("another account's device is outside the project");
    assert!(foreign.to_string().contains("does not own"));

    // A device push is owned by the account, so account reads and session end see it.
    store.start_session_for_account(&device.token, "device-session", context())?;
    store.sync_push(
        &device.token,
        observation_for("device-session", "device-event", "device pushed marker"),
    )?;
    assert!(store.observation_owned_by("device-event", &owner.account.person_id));
    store.end_session_for_account(&owner.token, "device-session")?;

    // Web-only actions refuse a device token with the web-only error.
    let web_only = [
        store.authenticate_session(&device.token).map(|_| ()),
        store.audit_events(&device.token, 10).map(|_| ()),
        store
            .rotate_device_token(&device.token, "device-own")
            .map(|_| ()),
        store.revoke_device(&device.token, "device-own").map(|_| ()),
        store.logout_account(&device.token),
    ];
    for result in web_only {
        let error = result.expect_err("device token is refused");
        assert!(error.to_string().contains(WEB_ONLY_ERROR));
    }
    assert!(store.authenticate_session(&owner.token).is_ok());

    // A solo project merges with a device token; a shared project does not.
    let branch =
        store.register_branch(&device.token, &project_id, "feat/auth", None, None, None)?;
    let source = remember_with_context(&mut store, &device.token, "solo merge source")?;
    store.confirm_branch_merge(
        &device.token,
        &project_id,
        &branch.branch_id,
        &source.memory_id,
        "c1",
        "solo",
    )?;
    join_project(&mut store, &other.token, &project_id)?;
    let shared_branch =
        store.register_branch(&owner.token, &project_id, "feat/shared", None, None, None)?;
    let mut shared_context = context();
    shared_context.branch = Some("feat/shared".to_owned());
    let shared_source = store.remember_for_account(
        &owner.token,
        "shared merge source",
        Vec::new(),
        shared_context,
        LifecycleInput::default(),
        MemoryKind::Explicit,
    )?;
    let refused = store
        .confirm_branch_merge(
            &device.token,
            &project_id,
            &shared_branch.branch_id,
            &shared_source.memory_id,
            "c2",
            "shared",
        )
        .expect_err("a shared project needs the web console");
    assert!(refused.to_string().contains(WEB_ONLY_ERROR));
    store.confirm_branch_merge(
        &owner.token,
        &project_id,
        &shared_branch.branch_id,
        &shared_source.memory_id,
        "c2",
        "shared",
    )?;

    // A revoked token is rejected, and use is recorded only for an active one.
    store.touch_device(&device.token);
    let used = store
        .devices()
        .find(|candidate| candidate.device_id == "device-own")
        .map(|candidate| candidate.last_seen_at.clone());
    assert!(used.is_some());
    store.revoke_device(&owner.token, "device-own")?;
    let revoked = store
        .authenticate_account(&device.token)
        .expect_err("revoked device token");
    assert!(revoked.to_string().contains("device is revoked"));
    assert!(store.is_device_token(&device.token));
    assert!(!store.is_device_token(&owner.token));

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn device_tokens_are_stored_hashed_and_reregistering_rotates() -> Result<(), StoreError> {
    let path = temp_store_path("device-token-hash");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let person_id = owner.account.person_id.clone();
    let first = store.register_device("device-a", person_id.clone(), None, "Laptop")?;
    let second = store.register_device("device-a", person_id, None, "Laptop")?;
    assert!(second.token.starts_with(DEVICE_TOKEN_PREFIX));
    assert_ne!(first.token, second.token);
    assert!(store.authenticate_device(&first.token).is_err());
    assert!(store.authenticate_device(&second.token).is_ok());
    assert!(!store.is_device_token(&first.token));

    let on_disk = fs::read_to_string(&path)?;
    assert!(!on_disk.contains(&first.token));
    assert!(!on_disk.contains(&second.token));
    assert!(on_disk.contains(&hash_token(&second.token)));
    let reopened = MemoryStore::open(&path)?;
    assert!(reopened.is_device_token(&second.token));
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn sync_refuses_a_device_of_a_disabled_account() -> Result<(), StoreError> {
    let path = temp_store_path("device-disabled-account");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let device = store.register_device("device-a", &owner.account.person_id, None, "Laptop")?;
    store.state.accounts[0].account.status = AccountStatus::Disabled;
    assert!(store.sync_pull(&device.token, 0).is_err());
    assert!(store.sync_diagnostics(&device.token).is_err());
    assert!(
        store
            .sync_push(
                &device.token,
                observation_for("s", "disabled-event", "nope")
            )
            .is_err()
    );
    assert!(store.authenticate_account(&device.token).is_err());
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn read_only_sync_calls_do_not_rewrite_the_store() -> Result<(), StoreError> {
    let path = temp_store_path("sync-read-only");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let device = store.register_device("device-a", &owner.account.person_id, None, "Laptop")?;
    // Pretend the last use was long ago so only the throttle could skip the write.
    store.state.devices[0].device.last_seen_at = "1".to_owned();
    store.persist()?;
    let before = fs::read(&path)?;

    store.sync_pull(&device.token, 0)?;
    store.sync_diagnostics(&device.token)?;
    assert_eq!(fs::read(&path)?, before);

    // A fresh last-seen value is not rewritten within the throttle window.
    store.touch_device(&device.token);
    let seen = store.state.devices[0].device.last_seen_at.clone();
    assert_ne!(seen, "1");
    store.touch_device(&device.token);
    assert_eq!(store.state.devices[0].device.last_seen_at, seen);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn revoked_device_cannot_be_registered_again() -> Result<(), StoreError> {
    let path = temp_store_path("revoked-device-register");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let person_id = owner.account.person_id.clone();
    let device = store.register_device("device-a", person_id.clone(), None, "Laptop")?;
    store.revoke_device(&owner.token, "device-a")?;
    let error = store
        .register_device("device-a", person_id, None, "Laptop")
        .expect_err("a revoked device id is not reusable");
    assert!(
        error
            .to_string()
            .contains("device is revoked; register a new device id")
    );
    assert!(store.authenticate_device(&device.token).is_err());
    fs::remove_file(path)?;
    Ok(())
}
