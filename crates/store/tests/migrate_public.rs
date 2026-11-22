//! Its own test binary: it plants `public.schema_version`, which other tests must not see.

use cairn_store::Store;
use cairn_store::testing::TestDb;

/// A `schema_version` table in `public` says nothing about a new schema: migrate must still apply.
#[tokio::test]
async fn a_schema_version_table_in_public_does_not_skip_the_migration() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    sqlx::raw_sql(
        "CREATE TABLE public.schema_version (version text PRIMARY KEY);
         INSERT INTO public.schema_version VALUES ('0001_foundation')",
    )
    .execute(db.store.pool())
    .await
    .unwrap();
    let schema = format!("t_public_{}", std::process::id());
    let opened = Store::open(db.url(), &schema).await;
    let has_memories = match &opened {
        Ok(store) => sqlx::query_scalar::<_, bool>("SELECT to_regclass($1) IS NOT NULL")
            .bind(format!("{schema}.memories"))
            .fetch_one(store.pool())
            .await
            .unwrap(),
        Err(_) => false,
    };
    sqlx::raw_sql(&format!(
        "DROP TABLE public.schema_version; DROP SCHEMA IF EXISTS {schema} CASCADE"
    ))
    .execute(db.store.pool())
    .await
    .unwrap();
    opened.expect("open");
    assert!(has_memories, "the migration ran in the new schema");
    db.finish().await;
}
