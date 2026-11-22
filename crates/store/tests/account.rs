use cairn_store::StoreError;
use cairn_store::testing::TestDb;

const PASSWORD: &str = "correct horse battery staple";

#[tokio::test]
async fn bootstrap_register_login_session_and_password() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let admin = store
        .bootstrap("Admin@Example.test", PASSWORD)
        .await
        .unwrap();
    assert!(admin.account.is_admin);
    assert_eq!(admin.account.email, "admin@example.test");
    assert!(
        store
            .bootstrap("other@example.test", PASSWORD)
            .await
            .is_err(),
        "one bootstrap only"
    );

    let user = store.register("user@example.test", PASSWORD).await.unwrap();
    assert!(!user.is_admin);
    assert!(
        store.register("USER@example.test", PASSWORD).await.is_err(),
        "email is unique"
    );
    assert!(store.register("short@example.test", "short").await.is_err());

    assert!(matches!(
        store.login("user@example.test", "wrong password!!").await,
        Err(StoreError::Forbidden(_))
    ));
    let session = store.login("user@example.test", PASSWORD).await.unwrap();
    assert_eq!(
        store
            .session_account(&session.token)
            .await
            .unwrap()
            .person_id,
        user.person_id
    );

    store
        .change_password(&user.person_id, PASSWORD, "another long password")
        .await
        .unwrap();
    assert!(
        store.session_account(&session.token).await.is_err(),
        "password change revokes sessions"
    );
    assert!(store.login("user@example.test", PASSWORD).await.is_err());
    let again = store
        .login("user@example.test", "another long password")
        .await
        .unwrap();
    store.logout(&again.token).await.unwrap();
    assert!(store.session_account(&again.token).await.is_err());
    assert_eq!(store.list_accounts().await.unwrap().len(), 2);
    db.finish().await;
}

#[tokio::test]
async fn devices_register_rotate_revoke_and_audit() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let me = db.person("me").await;
    let other = db.person("other").await;

    let first = store
        .register_device(&me, Some("laptop"), "MacBook")
        .await
        .unwrap();
    let (device, account) = store.authenticate_device(&first.token).await.unwrap();
    assert_eq!(
        (device.device_id.as_str(), account.person_id.as_str()),
        ("laptop", me.as_str())
    );
    assert!(
        store
            .authenticate_device("not-a-device-token")
            .await
            .is_err()
    );

    let rotated = store.rotate_device(&me, "laptop").await.unwrap();
    assert_ne!(rotated.token, first.token);
    assert!(
        store.authenticate_device(&first.token).await.is_err(),
        "old token stops working"
    );
    assert!(store.authenticate_device(&rotated.token).await.is_ok());
    assert!(
        store
            .register_device(&other, Some("laptop"), "x")
            .await
            .is_err(),
        "another person's id"
    );

    store.revoke_device(&me, "laptop").await.unwrap();
    assert!(store.authenticate_device(&rotated.token).await.is_err());
    assert!(
        store
            .register_device(&me, Some("laptop"), "x")
            .await
            .is_err(),
        "revoked ids stay revoked"
    );
    assert_eq!(store.list_devices(&me).await.unwrap().len(), 1);

    store
        .record_audit(
            &me,
            Some("laptop"),
            "device_revoked",
            Some("laptop"),
            serde_json::json!({"n": 1}),
        )
        .await
        .unwrap();
    let audit = store.list_audit(&me, &Default::default()).await.unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].action, "device_revoked");
    assert!(
        store
            .list_audit(&other, &Default::default())
            .await
            .unwrap()
            .is_empty()
    );
    db.finish().await;
}
