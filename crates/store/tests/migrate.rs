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
            "0005_console",
            "0006_agent_instructions",
            "0007_memory_rank"
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

/// A database at the pre-rank schema (0001–0006) already has rows. Adding `rank` keeps every row
/// and backfills `normal`.
#[tokio::test]
async fn rank_backfill_keeps_rows_normal() {
    let Ok(url) = std::env::var("CAIRN_TEST_DATABASE_URL") else {
        assert!(
            std::env::var("CAIRN_TEST_REQUIRE_DB").as_deref() != Ok("1"),
            "database required"
        );
        println!("SKIP (no CAIRN_TEST_DATABASE_URL)");
        return;
    };
    let schema = format!("t_rank_{}", std::process::id());
    let store = Store::connect(&url, &schema).await.expect("connect");
    sqlx::raw_sql(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::raw_sql(&format!("CREATE SCHEMA {schema}"))
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::raw_sql("CREATE EXTENSION IF NOT EXISTS vector SCHEMA public")
        .execute(store.pool())
        .await
        .unwrap();
    for (version, file) in [
        ("0001_foundation", "0001_foundation.sql"),
        ("0002_capture", "0002_capture.sql"),
        ("0003_retrieval", "0003_retrieval.sql"),
        ("0004_processing", "0004_processing.sql"),
        ("0005_console", "0005_console.sql"),
        ("0006_agent_instructions", "0006_agent_instructions.sql"),
    ] {
        let sql = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../migrations")
                .join(file),
        )
        .unwrap_or_else(|error| panic!("read {file}: {error}"));
        sqlx::raw_sql(&sql).execute(store.pool()).await.unwrap();
        sqlx::query("INSERT INTO schema_version (version) VALUES ($1)")
            .bind(version)
            .execute(store.pool())
            .await
            .unwrap();
    }
    sqlx::query(
        "INSERT INTO accounts (person_id, email, password_hash, is_admin)
         VALUES ('per_old', 'old@example.test', 'x', true)",
    )
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO memories (memory_id, owner_id, scope, kind, title, summary, evidence_hash, identity_hash, origin)
         VALUES ('mem_agent', 'per_old', 'personal', 'fact', 'Agent fact kept', 'An agent fact that already existed.', 'e1', 'i1', 'agent'),
                ('mem_system', 'per_old', 'personal', 'gotcha', 'System gotcha kept', 'A system gotcha that already existed.', 'e2', 'i2', 'system')",
    )
    .execute(store.pool())
    .await
    .unwrap();
    let before: Vec<String> =
        sqlx::query_scalar("SELECT memory_id FROM memories ORDER BY memory_id")
            .fetch_all(store.pool())
            .await
            .unwrap();
    assert_eq!(before, ["mem_agent", "mem_system"]);
    store.migrate().await.expect("apply rank migration");
    let after: Vec<(String, String)> =
        sqlx::query_as("SELECT memory_id, rank FROM memories ORDER BY memory_id")
            .fetch_all(store.pool())
            .await
            .unwrap();
    assert_eq!(
        after,
        [
            ("mem_agent".to_owned(), "normal".to_owned()),
            ("mem_system".to_owned(), "normal".to_owned()),
        ]
    );
    store.migrate().await.expect("idempotent");
    let again: Vec<(String, String)> =
        sqlx::query_as("SELECT memory_id, rank FROM memories ORDER BY memory_id")
            .fetch_all(store.pool())
            .await
            .unwrap();
    assert_eq!(again, after);
    sqlx::raw_sql(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(store.pool())
        .await
        .unwrap();
    store.pool().close().await;
}
