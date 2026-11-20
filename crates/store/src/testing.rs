//! `TestDb`: one schema per test inside `CAIRN_TEST_DATABASE_URL`, so tests run in parallel
//! against one container. Enabled by the `testing` feature.

use crate::Store;
use cairn_core::id::{new_id, sha256_hex};

pub struct TestDb {
    pub store: Store,
    url: String,
}

impl TestDb {
    /// A migrated, empty schema `t_<16 hex>`. With no database configured the test prints
    /// `SKIP` and gets `None`; with `CAIRN_TEST_REQUIRE_DB=1` (CI) it panics instead.
    pub async fn new() -> Option<Self> {
        let Ok(url) = std::env::var("CAIRN_TEST_DATABASE_URL") else {
            assert!(
                std::env::var("CAIRN_TEST_REQUIRE_DB").as_deref() != Ok("1"),
                "CAIRN_TEST_REQUIRE_DB=1 but CAIRN_TEST_DATABASE_URL is not set"
            );
            println!("SKIP (no CAIRN_TEST_DATABASE_URL)");
            return None;
        };
        let schema = format!("t_{}", &sha256_hex(&[&new_id("t")])[..16]);
        let store = Store::open(&url, &schema)
            .await
            .expect("connect and migrate test schema");
        Some(Self { store, url })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// Drops the schema. A panicking test leaks its schema, which is harmless.
    pub async fn finish(self) {
        let schema = self.store.schema().to_owned();
        sqlx::raw_sql(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
            .execute(self.store.pool())
            .await
            .expect("drop test schema");
        self.store.pool().close().await;
    }
}

impl TestDb {
    /// A new account. The first one in the schema is bootstrapped as the admin.
    pub async fn person(&self, name: &str) -> String {
        let email = format!("{name}@example.test");
        let password = "correct horse battery staple";
        if self
            .store
            .list_accounts()
            .await
            .expect("list accounts")
            .is_empty()
        {
            self.store
                .bootstrap(&email, password)
                .await
                .expect("bootstrap")
                .account
                .person_id
        } else {
            self.store
                .register(&email, password)
                .await
                .expect("register")
                .person_id
        }
    }
}

/// A `SaveInput` from JSON, for tests.
pub fn save_input(value: serde_json::Value) -> cairn_core::memory::SaveInput {
    serde_json::from_value(value).expect("valid save input")
}

/// A rule with a distinct title, which is enough for most tests.
pub fn rule(title: &str) -> cairn_core::memory::SaveInput {
    save_input(
        serde_json::json!({"kind": "rule", "title": title, "summary": format!("{title} summary"), "reason": "because"}),
    )
}
