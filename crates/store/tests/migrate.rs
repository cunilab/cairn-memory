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
            "0007_memory_rank",
            "0008_proof_count",
            "0009_memory_aliases",
            "0012_always_active"
        ]
    );
    let tables: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.tables WHERE table_schema = current_schema() AND table_type = 'BASE TABLE'",
    )
    .fetch_one(db.store.pool())
    .await
    .unwrap();
    assert_eq!(tables, 18);
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

/// A 0.1.8 database (0001–0006) already has rows and supersede links. `proof_count` keeps every
/// row, counts only automatic supersedes (`source_id` is the newer side), and a second run
/// changes nothing. A handoff chain (`origin = 'agent'`) adds no proof.
#[tokio::test]
async fn proof_backfill_idempotent() {
    let Ok(url) = std::env::var("CAIRN_TEST_DATABASE_URL") else {
        assert!(
            std::env::var("CAIRN_TEST_REQUIRE_DB").as_deref() != Ok("1"),
            "database required"
        );
        println!("SKIP (no CAIRN_TEST_DATABASE_URL)");
        return;
    };
    let schema = format!("t_proof_{}", std::process::id());
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
         VALUES
           ('mem_new', 'per_old', 'personal', 'gotcha', 'Use node 20', 'Use node 20 for the desktop app today.', 'e1', 'i1', 'agent'),
           ('mem_old', 'per_old', 'personal', 'gotcha', 'Use node 20 once', 'An older note of the same gotcha.', 'e2', 'i2', 'agent'),
           ('mem_old2', 'per_old', 'personal', 'gotcha', 'Use node 20 twice', 'Another older note of the same gotcha.', 'e3', 'i3', 'agent'),
           ('mem_hand_new', 'per_old', 'personal', 'handoff', 'Session today', 'The latest handoff on this checkout.', 'e4', 'i4', 'system'),
           ('mem_hand_old', 'per_old', 'personal', 'handoff', 'Session yesterday', 'The previous handoff on this checkout.', 'e5', 'i5', 'system'),
           ('mem_plain', 'per_old', 'personal', 'fact', 'Plain fact kept', 'A fact with no supersede link at all.', 'e6', 'i6', 'agent')",
    )
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO links (source_id, target_id, relation, origin) VALUES
           ('mem_new', 'mem_old', 'supersedes', 'auto'),
           ('mem_new', 'mem_old2', 'supersedes', 'auto'),
           ('mem_hand_new', 'mem_hand_old', 'supersedes', 'agent')",
    )
    .execute(store.pool())
    .await
    .unwrap();
    let before: Vec<String> =
        sqlx::query_scalar("SELECT memory_id FROM memories ORDER BY memory_id")
            .fetch_all(store.pool())
            .await
            .unwrap();
    store.migrate().await.expect("apply proof migration");
    let after: Vec<(String, i32)> =
        sqlx::query_as("SELECT memory_id, proof_count FROM memories ORDER BY memory_id")
            .fetch_all(store.pool())
            .await
            .unwrap();
    let ids: Vec<String> = after.iter().map(|(id, _)| id.clone()).collect();
    assert_eq!(ids, before, "no row lost");
    assert!(after.iter().all(|(_, proof)| *proof >= 1));
    assert_eq!(
        after,
        [
            ("mem_hand_new".into(), 1),
            ("mem_hand_old".into(), 1),
            ("mem_new".into(), 3),
            ("mem_old".into(), 1),
            ("mem_old2".into(), 1),
            ("mem_plain".into(), 1),
        ]
    );
    store.migrate().await.expect("idempotent");
    let again: Vec<(String, i32)> =
        sqlx::query_as("SELECT memory_id, proof_count FROM memories ORDER BY memory_id")
            .fetch_all(store.pool())
            .await
            .unwrap();
    assert_eq!(again, after);
    sqlx::query("UPDATE memories SET proof_count = 9 WHERE memory_id = 'mem_new'")
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE memories m SET proof_count = 1 + s.n
           FROM (SELECT source_id, count(*) n FROM links
                  WHERE relation = 'supersedes' AND origin = 'auto' GROUP BY 1) s
          WHERE m.memory_id = s.source_id AND m.proof_count < 1 + s.n",
    )
    .execute(store.pool())
    .await
    .unwrap();
    let kept: i32 =
        sqlx::query_scalar("SELECT proof_count FROM memories WHERE memory_id = 'mem_new'")
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(kept, 9, "backfill never lowers");
    sqlx::raw_sql(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(store.pool())
        .await
        .unwrap();
    store.pool().close().await;
}

#[tokio::test]
async fn test_0012_is_additive_on_populated_db() {
    let Ok(url) = std::env::var("CAIRN_TEST_DATABASE_URL") else {
        assert!(
            std::env::var("CAIRN_TEST_REQUIRE_DB").as_deref() != Ok("1"),
            "database required"
        );
        println!("SKIP (no CAIRN_TEST_DATABASE_URL)");
        return;
    };
    let schema = format!("t_always_{}", std::process::id());
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
        ("0007_memory_rank", "0007_memory_rank.sql"),
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
        "INSERT INTO memories (memory_id, owner_id, scope, kind, title, summary, evidence_hash, identity_hash, origin, status)
         VALUES ('mem_live', 'per_old', 'personal', 'fact', 'Live fact kept', 'A fact that already existed here.', 'e1', 'i1', 'agent', 'active'),
                ('mem_old', 'per_old', 'personal', 'fact', 'Old fact kept', 'A superseded fact that already existed.', 'e2', 'i2', 'agent', 'superseded')",
    )
    .execute(store.pool())
    .await
    .unwrap();
    let before: Vec<String> =
        sqlx::query_scalar("SELECT memory_id FROM memories ORDER BY memory_id")
            .fetch_all(store.pool())
            .await
            .unwrap();
    let count_before: i64 = sqlx::query_scalar("SELECT count(*) FROM memories")
        .fetch_one(store.pool())
        .await
        .unwrap();
    store.migrate().await.expect("apply 0012");
    let after: Vec<String> =
        sqlx::query_scalar("SELECT memory_id FROM memories ORDER BY memory_id")
            .fetch_all(store.pool())
            .await
            .unwrap();
    let count_after: i64 = sqlx::query_scalar("SELECT count(*) FROM memories")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(count_after, count_before);
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT indexname FROM pg_indexes WHERE schemaname = current_schema() AND indexname LIKE 'memories_embedding_hnsw%' ORDER BY 1",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert_eq!(
        indexes,
        ["memories_embedding_hnsw", "memories_embedding_hnsw_always"]
    );
    store.migrate().await.expect("idempotent");
    sqlx::raw_sql(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(store.pool())
        .await
        .unwrap();
    store.pool().close().await;
}

/// A 0.1.8 database (0001–0006) already has rows. Alias backfill adds one derived title per
/// non-forgotten memory with a non-empty title and one clipped symptom for a gotcha, leaves
/// forgotten rows and empty titles alone, and a second run inserts nothing. Memory rows are
/// not rewritten.
#[tokio::test]
async fn alias_backfill_idempotent() {
    let Ok(url) = std::env::var("CAIRN_TEST_DATABASE_URL") else {
        assert!(
            std::env::var("CAIRN_TEST_REQUIRE_DB").as_deref() != Ok("1"),
            "database required"
        );
        println!("SKIP (no CAIRN_TEST_DATABASE_URL)");
        return;
    };
    let schema = format!("t_alias_{}", std::process::id());
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
    let symptom = "s".repeat(130);
    sqlx::query(
        "INSERT INTO memories (memory_id, owner_id, scope, kind, title, summary, fields, evidence_hash, identity_hash, origin, status)
         VALUES
           ('mem_fact', 'per_old', 'personal', 'fact', 'Fact title kept', 'A fact that already existed.', '{}', 'e1', 'i1', 'agent', 'active'),
           ('mem_gotcha', 'per_old', 'personal', 'gotcha', 'Gotcha title kept', 'A gotcha that already existed.', $1, 'e2', 'i2', 'agent', 'active'),
           ('mem_empty', 'per_old', 'personal', 'gotcha', 'Empty symptom kept', 'A gotcha with no symptom text.', '{\"symptom\":\"\"}', 'e3', 'i3', 'agent', 'active'),
           ('mem_forgot', 'per_old', 'personal', 'gotcha', 'Forgotten gotcha', 'A forgotten gotcha with a symptom.', '{\"symptom\":\"still there\"}', 'e4', 'i4', 'agent', 'forgotten'),
           ('mem_note', 'per_old', 'personal', 'note', 'Note title kept', 'A note that already existed.', '{}', 'e5', 'i5', 'agent', 'active'),
           ('mem_blank', 'per_old', 'personal', 'fact', '', 'A memory whose title is empty.', '{}', 'e6', 'i6', 'agent', 'active')",
    )
    .bind(serde_json::json!({ "symptom": symptom }))
    .execute(store.pool())
    .await
    .unwrap();
    let before: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT memory_id, title, summary, fields::text, status FROM memories ORDER BY memory_id",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    store.migrate().await.expect("apply alias migration");
    let after: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT memory_id, title, summary, fields::text, status FROM memories ORDER BY memory_id",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert_eq!(after, before, "memory rows unchanged");
    let rows: Vec<(String, i16, String, String)> = sqlx::query_as(
        "SELECT memory_id, ord, source, text FROM memory_aliases ORDER BY memory_id, ord",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert_eq!(
        rows,
        [
            (
                "mem_empty".into(),
                100,
                "derived".into(),
                "Empty symptom kept".into()
            ),
            (
                "mem_fact".into(),
                100,
                "derived".into(),
                "Fact title kept".into()
            ),
            (
                "mem_gotcha".into(),
                100,
                "derived".into(),
                "Gotcha title kept".into()
            ),
            ("mem_gotcha".into(), 101, "derived".into(), "s".repeat(120)),
            (
                "mem_note".into(),
                100,
                "derived".into(),
                "Note title kept".into()
            ),
        ]
    );
    assert!(
        !rows.iter().any(|(id, _, _, _)| id == "mem_blank"),
        "an empty title is not copied into an alias"
    );
    let sql = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../migrations/0009_memory_aliases.sql"),
    )
    .unwrap();
    for statement in sql.split(';') {
        let statement = statement
            .lines()
            .filter(|line| !line.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");
        if statement.trim().to_ascii_uppercase().starts_with("INSERT") {
            sqlx::raw_sql(statement.trim())
                .execute(store.pool())
                .await
                .unwrap();
        }
    }
    let again: Vec<(String, i16, String, String)> = sqlx::query_as(
        "SELECT memory_id, ord, source, text FROM memory_aliases ORDER BY memory_id, ord",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert_eq!(again, rows, "second run inserts 0");
    sqlx::query(
        "INSERT INTO memory_aliases (memory_id, ord, source, text)
         VALUES ('mem_fact', 50, 'learned', 'a learned way to ask this')",
    )
    .execute(store.pool())
    .await
    .expect("learned is an allowed source");
    sqlx::raw_sql(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(store.pool())
        .await
        .unwrap();
    store.pool().close().await;
}
