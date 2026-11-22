use cairn_store::testing::TestDb;
use cairn_store::{Store, StoreError};

#[tokio::test]
async fn migrate_twice_is_a_no_op() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    db.store.migrate().await.expect("second migrate");
    db.store.migrate().await.expect("third migrate");
    let versions: Vec<String> = sqlx::query_scalar("SELECT version FROM schema_version")
        .fetch_all(db.store.pool())
        .await
        .unwrap();
    assert_eq!(
        versions,
        [
            "0001_foundation",
            "0002_capture",
            "0003_retrieval",
            "0004_processing",
            "0005_console"
        ]
    );
    let tables: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.tables WHERE table_schema = current_schema() AND table_type = 'BASE TABLE'",
    )
    .fetch_one(db.store.pool())
    .await
    .unwrap();
    assert_eq!(tables, 17);
    // Everything lives in the test schema, nothing leaks into public.
    let public: i64 = sqlx::query_scalar("SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public' AND table_name IN ('memories','accounts','jobs')")
        .fetch_one(db.store.pool())
        .await
        .unwrap();
    assert_eq!(public, 0);
    db.finish().await;
}

#[tokio::test]
async fn parallel_migrations_into_separate_schemas_do_not_race() {
    let Some(first) = TestDb::new().await else {
        return;
    };
    let url = first.url().to_owned();
    let open = |i: u32| {
        let url = url.clone();
        async move {
            let schema = format!("t_race_{}_{i}", std::process::id());
            let store = Store::open(&url, &schema).await.expect("open");
            sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
                .execute(store.pool())
                .await
                .unwrap();
        }
    };
    tokio::join!(open(0), open(1), open(2), open(3));
    first.finish().await;
}

/// D2: a database holding `public.cairn_memories` is refused, never dropped. Uses its own
/// database so no other test sees the legacy table.
#[tokio::test]
async fn pre_0_1_6_database_is_refused() {
    let Ok(url) = std::env::var("CAIRN_TEST_DATABASE_URL") else {
        assert!(
            std::env::var("CAIRN_TEST_REQUIRE_DB").as_deref() != Ok("1"),
            "database required"
        );
        println!("SKIP (no CAIRN_TEST_DATABASE_URL)");
        return;
    };
    let (base, tail) = url.rsplit_once('/').expect("url has a database");
    let query = tail.split_once('?').map_or("", |(_, q)| q);
    let with_db = |name: &str| {
        if query.is_empty() {
            format!("{base}/{name}")
        } else {
            format!("{base}/{name}?{query}")
        }
    };
    let admin = Store::connect(&url, "public").await.unwrap();
    let name = format!("cairn_legacy_{}", std::process::id());
    sqlx::raw_sql(&format!("CREATE DATABASE {name}"))
        .execute(admin.pool())
        .await
        .expect("create database");

    let legacy = Store::connect(&with_db(&name), "cairn").await.unwrap();
    sqlx::raw_sql("CREATE TABLE public.cairn_memories (memory_id text)")
        .execute(legacy.pool())
        .await
        .unwrap();
    let error = legacy.migrate().await.expect_err("refused");
    assert!(matches!(error, StoreError::LegacyDatabase));
    assert_eq!(error.to_string(), "pre-0.1.6 database; recreate it");
    let schemas =
        sqlx::query("SELECT 1 FROM information_schema.schemata WHERE schema_name = 'cairn'")
            .fetch_optional(legacy.pool())
            .await
            .unwrap();
    assert!(schemas.is_none(), "nothing was created");
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM public.cairn_memories")
        .fetch_one(legacy.pool())
        .await
        .unwrap();
    assert_eq!(kept, 0);
    legacy.pool().close().await;
    sqlx::raw_sql(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .execute(admin.pool())
        .await
        .unwrap();
}
