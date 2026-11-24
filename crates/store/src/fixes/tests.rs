use crate::testing::{TestDb, save_input};
use cairn_core::capture::WorkContext;

#[tokio::test]
async fn learned_alias_validates_and_redacts() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let owner = db.person("owner").await;
    let memory = db.store.save(&owner, save_input(serde_json::json!({"kind":"fact", "title":"Alias test memory", "summary":"A complete fact for the alias test", "reason":"Test alias validation"})), &WorkContext::default()).await.unwrap().id;
    let mut tx = db.store.pool().begin().await.unwrap();
    super::apply_add_alias(&mut tx, &memory, "password=secretvalue")
        .await
        .unwrap();
    assert!(
        super::apply_add_alias(&mut tx, &memory, "bad\nphrase")
            .await
            .is_err()
    );
    let (source, phrase): (String, String) = sqlx::query_as(
        "SELECT source,text FROM memory_aliases WHERE memory_id=$1 AND source='learned'",
    )
    .bind(&memory)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(source, "learned");
    assert_eq!(phrase, "password=[REDACTED]");
    tx.rollback().await.unwrap();
    db.finish().await;
}
