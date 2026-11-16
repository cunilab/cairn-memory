use crate::accounts::{ADMIN_ONLY_ERROR, WEB_ONLY_ERROR};
use crate::store::MemoryStore;
use crate::test_support::temp_store_path;
use crate::types::StoreError;
use std::fs;

#[test]
fn account_auth_hashes_credentials_and_persists_revocable_sessions() -> Result<(), StoreError> {
    let path = temp_store_path("account-auth");
    let password = "correct horse battery staple";
    let mut store = MemoryStore::open(&path)?;
    let bootstrap = store.bootstrap_account("Owner@Example.test", password)?;
    assert_eq!(bootstrap.account.email, "owner@example.test");
    assert!(store.authenticate_account(&bootstrap.token).is_ok());
    assert!(!fs::read_to_string(&path)?.contains(password));
    assert!(!fs::read_to_string(&path)?.contains(&bootstrap.token));

    let login = store.login("owner@example.test", password)?;
    assert_ne!(login.token, bootstrap.token);
    assert!(store.login("owner@example.test", "wrong password").is_err());
    assert!(
        store
            .bootstrap_account("second@example.test", password)
            .is_err()
    );

    drop(store);
    let mut restored = MemoryStore::open(&path)?;
    assert!(restored.authenticate_account(&login.token).is_ok());
    restored.logout_account(&login.token)?;
    assert!(restored.authenticate_account(&login.token).is_err());

    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn only_the_admin_web_session_creates_accounts() -> Result<(), StoreError> {
    let path = temp_store_path("admin-register");
    let mut store = MemoryStore::open(&path)?;
    let pw = "correct horse battery staple";
    let admin = store.bootstrap_account("admin@example.test", pw)?;
    assert!(admin.account.is_admin);
    assert!(store.authenticate_account(&admin.token)?.is_admin);

    let member = store.register_account_as_admin(&admin.token, "m@example.test", pw)?;
    assert!(!member.account.is_admin);
    assert!(!store.login("m@example.test", pw)?.account.is_admin);
    assert!(!store.authenticate_account(&member.token)?.is_admin);

    let denied = |error: StoreError, text: &str| error.to_string().contains(text);
    let error = store
        .register_account_as_admin(&member.token, "x@example.test", pw)
        .unwrap_err();
    assert!(denied(error, ADMIN_ONLY_ERROR));
    let device = store.register_device("dev-admin", &admin.account.person_id, None, "Hook")?;
    let error = store
        .register_account_as_admin(&device.token, "x@example.test", pw)
        .unwrap_err();
    assert!(denied(error, WEB_ONLY_ERROR));
    let error = store
        .register_account_as_admin("nope", "x@example.test", pw)
        .unwrap_err();
    assert!(denied(error, "account authentication failed"));
    assert!(
        store.login("x@example.test", pw).is_err(),
        "nothing was created"
    );

    // The flag survives a reload.
    let reopened = MemoryStore::open(&path)?;
    assert!(reopened.authenticate_account(&admin.token)?.is_admin);
    assert!(!reopened.authenticate_account(&member.token)?.is_admin);
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn only_the_admin_session_lists_accounts() -> Result<(), StoreError> {
    let path = temp_store_path("list-accounts");
    let mut store = MemoryStore::open(&path)?;
    let admin = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
    let user = store.register_account("u@example.test", "correct horse battery staple")?;
    let device = store.register_device("dev-a", &admin.account.person_id, None, "Laptop")?;

    let listed = store.accounts(&admin.token)?;
    assert_eq!(
        listed.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(),
        ["a@example.test", "u@example.test"]
    );
    assert!(listed[0].is_admin && !listed[1].is_admin);
    let refused = store.accounts(&user.token).expect_err("not admin");
    assert!(refused.to_string().contains(ADMIN_ONLY_ERROR));
    let refused = store.accounts(&device.token).expect_err("device token");
    assert!(refused.to_string().contains(WEB_ONLY_ERROR));
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn the_admin_resets_a_password_and_every_web_session_of_that_user_ends() -> Result<(), StoreError> {
    let path = temp_store_path("reset-password");
    let mut store = MemoryStore::open(&path)?;
    let admin = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
    let user = store.register_account("u@example.test", "correct horse battery staple")?;
    let second = store.login("u@example.test", "correct horse battery staple")?;
    let device = store.register_device("dev-u", &user.account.person_id, None, "Laptop")?;
    let fresh = "a brand new passphrase";

    for (token, expected) in [
        (&user.token, ADMIN_ONLY_ERROR),
        (&device.token, WEB_ONLY_ERROR),
    ] {
        let refused = store
            .reset_account_password(token, &user.account.person_id, fresh)
            .expect_err("admin web session only");
        assert!(refused.to_string().contains(expected));
    }
    assert!(
        store
            .reset_account_password(&admin.token, &user.account.person_id, "short")
            .is_err()
    );
    assert!(
        store
            .reset_account_password(&admin.token, "person-missing", fresh)
            .is_err()
    );
    store.authenticate_session(&user.token)?;

    store.reset_account_password(&admin.token, &user.account.person_id, fresh)?;
    for token in [&user.token, &second.token] {
        assert!(store.authenticate_session(token).is_err());
    }
    assert!(
        store.authenticate_account(&device.token).is_ok(),
        "devices stay valid"
    );
    assert!(
        store
            .login("u@example.test", "correct horse battery staple")
            .is_err()
    );
    let relogin = store.login("u@example.test", fresh)?;
    store.authenticate_session(&admin.token)?;
    assert!(
        store
            .audit_events(&relogin.token, 10)?
            .iter()
            .any(|event| event.action == "account_password_reset")
    );
    let on_disk = fs::read_to_string(&path)?;
    assert!(!on_disk.contains(fresh));
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn changing_the_password_needs_the_current_one_and_keeps_only_this_session()
-> Result<(), StoreError> {
    let path = temp_store_path("change-password");
    let mut store = MemoryStore::open(&path)?;
    let user = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
    let other = store.login("a@example.test", "correct horse battery staple")?;
    let device = store.register_device("dev-a", &user.account.person_id, None, "Laptop")?;
    let fresh = "a brand new passphrase";

    let wrong = store
        .change_password(&user.token, "not the password", fresh)
        .expect_err("wrong current password");
    assert!(wrong.to_string().contains("invalid account credentials"));
    assert!(
        store
            .change_password(&user.token, "correct horse battery staple", "short")
            .is_err()
    );
    let refused = store
        .change_password(&device.token, "correct horse battery staple", fresh)
        .expect_err("device token");
    assert!(refused.to_string().contains(WEB_ONLY_ERROR));
    store.authenticate_session(&other.token)?;

    store.change_password(&user.token, "correct horse battery staple", fresh)?;
    store.authenticate_session(&user.token)?;
    assert!(store.authenticate_session(&other.token).is_err());
    assert!(store.authenticate_account(&device.token).is_ok());
    assert!(
        store
            .login("a@example.test", "correct horse battery staple")
            .is_err()
    );
    store.login("a@example.test", fresh)?;
    fs::remove_file(path)?;
    Ok(())
}
