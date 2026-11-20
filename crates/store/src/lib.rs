//! PostgreSQL persistence for the 0.1.6 memory model. Everything lives in one schema
//! (`cairn` in production, `t_<random>` per test); see `migrations/foundation/`.

pub mod account;
pub mod capture;
pub mod consolidate;
pub mod device;
pub mod embed;
pub mod export;
pub mod jobs;
pub mod memory;
pub mod project;
pub mod retrieve;
#[cfg(feature = "testing")]
pub mod testing;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Executor, PgPool};
use std::str::FromStr;

/// `migrations/foundation/NNNN_*.sql`, applied in order. Each runs in one transaction.
const MIGRATIONS: &[(&str, &str)] = &[(
    "0001_foundation",
    include_str!("../../../migrations/foundation/0001_foundation.sql"),
)];

/// Held for the whole of every migration step, so parallel starts and tests do not race on
/// `CREATE EXTENSION`.
const MIGRATION_LOCK: i64 = 0x6361_6972;

#[derive(Debug)]
pub enum StoreError {
    Validation(String),
    Conflict(String),
    NotFound(String),
    Forbidden(String),
    /// D2: `public.cairn_memories` exists. The operator recreates the database.
    LegacyDatabase,
    Database(sqlx::Error),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(m) => write!(f, "validation error: {m}"),
            Self::Conflict(m) => write!(f, "conflict: {m}"),
            Self::NotFound(m) => write!(f, "not found: {m}"),
            Self::Forbidden(m) => write!(f, "forbidden: {m}"),
            Self::LegacyDatabase => f.write_str("pre-0.1.6 database; recreate it"),
            Self::Database(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<sqlx::Error> for StoreError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl From<cairn_core::error::Error> for StoreError {
    fn from(error: cairn_core::error::Error) -> Self {
        match error {
            cairn_core::error::Error::Validation(m) => Self::Validation(m),
            cairn_core::error::Error::Conflict(m) => Self::Conflict(m),
        }
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Clone)]
pub struct Store {
    pool: PgPool,
    schema: String,
}

impl Store {
    /// Opens a pool whose every connection has `search_path = <schema>, public`. Does not migrate.
    pub async fn connect(database_url: &str, schema: &str) -> Result<Self> {
        if schema.is_empty()
            || !schema
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(StoreError::Validation(
                "schema must be lower-case letters, digits and _".into(),
            ));
        }
        let options = PgConnectOptions::from_str(database_url)?;
        let search_path = format!("SET search_path = {schema}, public");
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .after_connect(move |conn, _| {
                let statement = search_path.clone();
                Box::pin(async move {
                    conn.execute(statement.as_str()).await?;
                    Ok(())
                })
            })
            .connect_with(options)
            .await?;
        Ok(Self {
            pool,
            schema: schema.to_owned(),
        })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    /// Creates the schema and extension, then applies each unapplied migration. Running it twice
    /// changes nothing. Refuses a pre-0.1.6 database (D2) and never drops data.
    pub async fn migrate(&self) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(MIGRATION_LOCK)
            .execute(&mut *tx)
            .await?;
        let legacy: bool =
            sqlx::query_scalar("SELECT to_regclass('public.cairn_memories') IS NOT NULL")
                .fetch_one(&mut *tx)
                .await?;
        if legacy {
            return Err(StoreError::LegacyDatabase);
        }
        sqlx::raw_sql(&format!("CREATE SCHEMA IF NOT EXISTS {}", self.schema))
            .execute(&mut *tx)
            .await?;
        sqlx::raw_sql("CREATE EXTENSION IF NOT EXISTS vector SCHEMA public")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        for (version, sql) in MIGRATIONS {
            let mut tx = self.pool.begin().await?;
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(MIGRATION_LOCK)
                .execute(&mut *tx)
                .await?;
            let has_table: bool =
                sqlx::query_scalar("SELECT to_regclass('schema_version') IS NOT NULL")
                    .fetch_one(&mut *tx)
                    .await?;
            let applied = has_table
                && sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS (SELECT 1 FROM schema_version WHERE version = $1)",
                )
                .bind(version)
                .fetch_one(&mut *tx)
                .await?;
            if !applied {
                sqlx::raw_sql(sql).execute(&mut *tx).await?;
                sqlx::query("INSERT INTO schema_version (version) VALUES ($1)")
                    .bind(version)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
        }
        Ok(())
    }

    /// `connect` then `migrate`.
    pub async fn open(database_url: &str, schema: &str) -> Result<Self> {
        let store = Self::connect(database_url, schema).await?;
        store.migrate().await?;
        Ok(store)
    }
}

/// Epoch seconds of a timestamptz column, for `Row::get::<i64, _>`.
macro_rules! epoch {
    ($column:literal) => {
        concat!("EXTRACT(EPOCH FROM ", $column, ")::bigint")
    };
}
pub(crate) use epoch;
