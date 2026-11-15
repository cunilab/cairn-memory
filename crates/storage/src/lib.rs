//! PostgreSQL persistence for the Layer 3 account and trusted-device slice.
//!
//! The local `cairn-core` file adapter remains useful for compatibility and
//! offline tests. This crate owns the async, server-authoritative boundary so
//! the transport can migrate without changing the account or sync contracts.

use cairn_core::{
    AUTH_SESSION_SECONDS, Account, AccountStatus, AuditEvent, AuthSessionResult, ContextPack,
    Device, DeviceRegistration, DeviceStatus, ExportBundle, ImportResult, Lifecycle,
    LifecycleInput, LifecycleSummary, Memory, MemoryKind, MemoryScope, Observation,
    ObservationInput, Project, ProjectBranch, ProjectConflict, ProjectEntity, ProjectEntityInput,
    ProjectFreshnessEvent, ProjectMaintenanceReport, ProjectMembership, ProjectMemoryShare,
    ProjectMergeResult, ProjectShareCandidate, ProjectSharePolicy, ProjectSharePolicyRecord,
    ProjectTeam, ProjectTeamMember, ProjectWorktree, RecallFilter, RecallResult, SHARE_REASON_AUTO,
    SHARE_REASON_MANUAL, Session, ShareCandidateDismissal, SharedProjectMemory, SyncDeletion,
    SyncDiagnostics, SyncEvent, SyncPullResult, SyncPushResult, WorkContext, build_context_pack,
    build_explicit_memory_record, carry_summary_state, competing_project_memories,
    derive_session_memory, memory_matches_filter, normalize_email, normalize_export_bundle,
    normalize_observation, now_seconds, prefer_newer_contradiction, random_token, recall_result,
    redact_memory_metadata, redact_sensitive_content, redact_work_context,
    sensitivity_including_context, session_memory_id, validate_password, why_changed_priority,
};
use sqlx::postgres::{PgPool, PgPoolOptions, PgRow};
use sqlx::types::Json;
use sqlx::{Postgres, Row, Transaction};
use std::collections::{HashMap, HashSet};
use std::fmt;

const MIGRATION: &str = include_str!("../../../migrations/0001_layer3_personal_memory.sql");
const AUDIT_MIGRATION: &str = include_str!("../../../migrations/0002_layer3_audit.sql");
const PROJECT_MIGRATION: &str = include_str!("../../../migrations/0003_layer4_projects.sql");
const FRESHNESS_MIGRATION: &str = include_str!("../../../migrations/0004_layer4_freshness.sql");
const PROJECT_ENTITIES_MIGRATION: &str =
    include_str!("../../../migrations/0005_layer4_project_entities.sql");
const BRANCH_RESOLUTION_MIGRATION: &str =
    include_str!("../../../migrations/0006_layer4_branch_resolution.sql");
const REGISTRY_MAINTENANCE_MIGRATION: &str =
    include_str!("../../../migrations/0007_layer4_registry_maintenance.sql");
const PROJECT_SHARE_MIGRATION: &str =
    include_str!("../../../migrations/0008_layer5_project_shares.sql");
const SHARE_POLICY_MIGRATION: &str =
    include_str!("../../../migrations/0009_layer5_share_policy.sql");
const TEAM_SPACE_MIGRATION: &str = include_str!("../../../migrations/0010_layer5_team_space.sql");
const SYNC_DELETION_MIGRATION: &str = include_str!("../../../migrations/0011_sync_deletions.sql");
const SESSION_PENDING_MIGRATION: &str =
    include_str!("../../../migrations/0012_session_summary_pending.sql");
const ADMIN_ROLE_MIGRATION: &str = include_str!("../../../migrations/0013_admin_role.sql");
const MAX_PULL_EVENTS: i64 = 512;
/// A session whose newest retained observation is older than this is finished on the next
/// capture or session start of the same person.
const IDLE_SESSION_SECONDS: i64 = 30 * 60;
/// Sessions one capture or start may finish. The rest wait for the next trigger.
const MAX_SWEEP_SESSIONS: i64 = 16;
const MAX_SEARCH_RESULTS: i64 = 512;

#[derive(Debug)]
pub enum PostgresError {
    Database(sqlx::Error),
    Core(cairn_core::StoreError),
    Json(serde_json::Error),
    Validation(String),
    /// An event id that is already taken by different content.
    Conflict(String),
    RateLimited,
}

impl PostgresError {
    pub fn is_unauthorized(&self) -> bool {
        let message = match self {
            Self::Validation(message) => message,
            Self::Core(cairn_core::StoreError::Validation(message)) => message,
            _ => return false,
        };
        message.contains("authentication failed")
            || message.contains("invalid account credentials")
            || message.contains("expired or revoked")
            || message.contains("account is disabled")
            || message.contains("account does not own")
            || message.contains("device is revoked")
            || message.contains("device is outside the caller scope")
            || message.contains("out-of-scope device")
    }
}

impl fmt::Display for PostgresError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "database error: {error}"),
            Self::Core(error) => write!(formatter, "core validation error: {error}"),
            Self::Json(error) => write!(formatter, "JSON encoding error: {error}"),
            Self::Validation(message) => write!(formatter, "validation error: {message}"),
            Self::Conflict(message) => write!(formatter, "conflict: {message}"),
            Self::RateLimited => write!(formatter, "authentication rate limit exceeded"),
        }
    }
}

impl std::error::Error for PostgresError {}

impl From<sqlx::Error> for PostgresError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl From<cairn_core::StoreError> for PostgresError {
    fn from(error: cairn_core::StoreError) -> Self {
        Self::Core(error)
    }
}

impl From<serde_json::Error> for PostgresError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Clone)]
pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    pub async fn connect(database_url: &str) -> Result<Self, PostgresError> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(database_url)
            .await?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn migrate(&self) -> Result<(), PostgresError> {
        sqlx::raw_sql(MIGRATION).execute(&self.pool).await?;
        sqlx::raw_sql(AUDIT_MIGRATION).execute(&self.pool).await?;
        sqlx::raw_sql(PROJECT_MIGRATION).execute(&self.pool).await?;
        sqlx::raw_sql(FRESHNESS_MIGRATION)
            .execute(&self.pool)
            .await?;
        sqlx::raw_sql(PROJECT_ENTITIES_MIGRATION)
            .execute(&self.pool)
            .await?;
        sqlx::raw_sql(BRANCH_RESOLUTION_MIGRATION)
            .execute(&self.pool)
            .await?;
        sqlx::raw_sql(REGISTRY_MAINTENANCE_MIGRATION)
            .execute(&self.pool)
            .await?;
        sqlx::raw_sql(PROJECT_SHARE_MIGRATION)
            .execute(&self.pool)
            .await?;
        sqlx::raw_sql(SHARE_POLICY_MIGRATION)
            .execute(&self.pool)
            .await?;
        sqlx::raw_sql(TEAM_SPACE_MIGRATION)
            .execute(&self.pool)
            .await?;
        sqlx::raw_sql(SYNC_DELETION_MIGRATION)
            .execute(&self.pool)
            .await?;
        sqlx::raw_sql(SESSION_PENDING_MIGRATION)
            .execute(&self.pool)
            .await?;
        sqlx::raw_sql(ADMIN_ROLE_MIGRATION)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn bootstrap_account(
        &self,
        email: &str,
        password: &str,
    ) -> Result<AuthSessionResult, PostgresError> {
        let email = normalize_email(email)?;
        validate_password(password)?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('cairn-account-bootstrap'))")
            .execute(&mut *transaction)
            .await?;
        let exists = sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM cairn_accounts)")
            .fetch_one(&mut *transaction)
            .await?;
        if exists {
            transaction.rollback().await?;
            return Err(PostgresError::Validation(
                "account bootstrap has already completed".to_owned(),
            ));
        }

        let person_id = format!(
            "person-{}",
            &cairn_core::hash_token(&random_token("person"))[..24]
        );
        let account = Account {
            person_id: person_id.clone(),
            email,
            status: AccountStatus::Active,
            is_admin: true,
            created_at: now_seconds().to_string(),
        };
        let password_hash = cairn_core::hash_password(password)?;
        let token = random_token("cairn-session");
        let token_hash = cairn_core::hash_token(&token);
        let session_id = format!("auth-{}", &token_hash[..16]);
        let expires_at = now_seconds() + AUTH_SESSION_SECONDS;
        sqlx::query(
            "INSERT INTO cairn_accounts (person_id, email, password_hash, is_admin)
             VALUES ($1, $2, $3, true)",
        )
        .bind(&person_id)
        .bind(&account.email)
        .bind(password_hash)
        .execute(&mut *transaction)
        .await?;
        insert_auth_session(
            &mut transaction,
            &session_id,
            &person_id,
            &token_hash,
            expires_at,
        )
        .await?;
        record_audit(
            &mut transaction,
            &person_id,
            None,
            "account_bootstrap",
            Some(&person_id),
            serde_json::json!({}),
        )
        .await?;
        transaction.commit().await?;

        Ok(AuthSessionResult {
            account,
            token,
            expires_at: expires_at.to_string(),
        })
    }

    pub async fn register_account(
        &self,
        email: &str,
        password: &str,
    ) -> Result<AuthSessionResult, PostgresError> {
        let email = normalize_email(email)?;
        validate_password(password)?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('cairn-account-bootstrap'))")
            .execute(&mut *transaction)
            .await?;
        let exists = sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM cairn_accounts)")
            .fetch_one(&mut *transaction)
            .await?;
        if !exists {
            transaction.rollback().await?;
            return Err(PostgresError::Validation(
                "account bootstrap has not completed".to_owned(),
            ));
        }
        let duplicate = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM cairn_accounts WHERE email = $1)",
        )
        .bind(&email)
        .fetch_one(&mut *transaction)
        .await?;
        if duplicate {
            transaction.rollback().await?;
            return Err(PostgresError::Validation(
                "account email is already registered".to_owned(),
            ));
        }
        let person_id = format!(
            "person-{}",
            &cairn_core::hash_token(&random_token("person"))[..24]
        );
        let account = Account {
            person_id: person_id.clone(),
            email,
            status: AccountStatus::Active,
            is_admin: false,
            created_at: now_seconds().to_string(),
        };
        let password_hash = cairn_core::hash_password(password)?;
        let token = random_token("cairn-session");
        let token_hash = cairn_core::hash_token(&token);
        let session_id = format!("auth-{}", &token_hash[..16]);
        let expires_at = now_seconds() + AUTH_SESSION_SECONDS;
        sqlx::query(
            "INSERT INTO cairn_accounts (person_id, email, password_hash) VALUES ($1, $2, $3)",
        )
        .bind(&person_id)
        .bind(&account.email)
        .bind(password_hash)
        .execute(&mut *transaction)
        .await?;
        insert_auth_session(
            &mut transaction,
            &session_id,
            &person_id,
            &token_hash,
            expires_at,
        )
        .await?;
        record_audit(
            &mut transaction,
            &person_id,
            None,
            "account_register",
            Some(&person_id),
            serde_json::json!({}),
        )
        .await?;
        transaction.commit().await?;
        Ok(AuthSessionResult {
            account,
            token,
            expires_at: expires_at.to_string(),
        })
    }

    /// Admin web session only. A device token fails with `WEB_ONLY_ERROR`, a non-admin
    /// session with `ADMIN_ONLY_ERROR`.
    pub async fn authenticate_admin(&self, token: &str) -> Result<Account, PostgresError> {
        let account = self.authenticate_session(token).await?;
        if account.is_admin {
            Ok(account)
        } else {
            Err(PostgresError::Validation(
                cairn_core::ADMIN_ONLY_ERROR.to_owned(),
            ))
        }
    }

    pub async fn register_account_as_admin(
        &self,
        token: &str,
        email: &str,
        password: &str,
    ) -> Result<AuthSessionResult, PostgresError> {
        self.authenticate_admin(token).await?;
        self.register_account(email, password).await
    }

    pub async fn login(
        &self,
        email: &str,
        password: &str,
    ) -> Result<AuthSessionResult, PostgresError> {
        let email = normalize_email(email)?;
        let row = sqlx::query(
            "SELECT person_id, email, password_hash, status, is_admin, EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at
             FROM cairn_accounts WHERE email = $1",
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| PostgresError::Validation("invalid account credentials".to_owned()))?;
        let password_hash: String = row.try_get("password_hash")?;
        let status: String = row.try_get("status")?;
        if status != "active" || !cairn_core::verify_password(password, &password_hash) {
            return Err(PostgresError::Validation(
                "invalid account credentials".to_owned(),
            ));
        }
        let account = account_from_row(&row)?;
        self.issue_session(account).await
    }

    /// Agent credential: a web session token or an active device token, resolved to the
    /// owning account.
    pub async fn authenticate_account(&self, token: &str) -> Result<Account, PostgresError> {
        self.authenticate_principal(token)
            .await
            .map(|(account, _)| account)
    }

    /// Web-only credential: a device token fails with `WEB_ONLY_ERROR`.
    pub async fn authenticate_session(&self, token: &str) -> Result<Account, PostgresError> {
        let error = match self.session_account(token).await {
            Ok(account) => return Ok(account),
            Err(error) => error,
        };
        if self.is_device_token(token).await? {
            return Err(PostgresError::Validation(
                cairn_core::WEB_ONLY_ERROR.to_owned(),
            ));
        }
        Err(error)
    }

    /// True for any device token, including a revoked one.
    pub async fn is_device_token(&self, token: &str) -> Result<bool, PostgresError> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM cairn_devices WHERE token_hash = $1)",
        )
        .bind(cairn_core::hash_token(token))
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn has_accounts(&self) -> Result<bool, PostgresError> {
        Ok(
            sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM cairn_accounts)")
                .fetch_one(&self.pool)
                .await?,
        )
    }

    /// The account and whether the credential was a device token. A device use also
    /// records `last_seen_at`.
    async fn authenticate_principal(&self, token: &str) -> Result<(Account, bool), PostgresError> {
        if let Ok(account) = self.session_account(token).await {
            return Ok((account, false));
        }
        // The touch skips a device seen within the throttle window; the read does not need it.
        let row = sqlx::query(
            "WITH touched AS (
                 UPDATE cairn_devices SET last_seen_at = now()
                 WHERE token_hash = $1 AND status = 'active'
                   AND last_seen_at < now() - interval '60 seconds'
             )
             SELECT a.person_id, a.email, a.status, a.is_admin,
                    EXTRACT(EPOCH FROM a.created_at)::bigint::text AS created_at
             FROM cairn_devices d JOIN cairn_accounts a ON a.person_id = d.person_id
             WHERE d.token_hash = $1 AND d.status = 'active' AND a.status = 'active'",
        )
        .bind(cairn_core::hash_token(token))
        .fetch_optional(&self.pool)
        .await?;
        if let Some(row) = row {
            return Ok((account_from_row(&row)?, true));
        }
        if self.is_device_token(token).await? {
            return Err(PostgresError::Validation("device is revoked".to_owned()));
        }
        Err(PostgresError::Validation(
            "account authentication failed".to_owned(),
        ))
    }

    async fn session_account(&self, token: &str) -> Result<Account, PostgresError> {
        let token_hash = cairn_core::hash_token(token);
        let row = sqlx::query(
            "SELECT a.person_id, a.email, a.status, a.is_admin,
                    EXTRACT(EPOCH FROM a.created_at)::bigint::text AS created_at
             FROM cairn_auth_sessions s
             JOIN cairn_accounts a ON a.person_id = s.person_id
             WHERE s.token_hash = $1 AND s.revoked_at IS NULL
               AND s.expires_at > now() AND a.status = 'active'",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| PostgresError::Validation("account authentication failed".to_owned()))?;
        account_from_row(&row)
    }

    pub async fn logout_account(&self, token: &str) -> Result<(), PostgresError> {
        if self.is_device_token(token).await? {
            return Err(PostgresError::Validation(
                cairn_core::WEB_ONLY_ERROR.to_owned(),
            ));
        }
        let token_hash = cairn_core::hash_token(token);
        let person_id = sqlx::query_scalar::<_, String>(
            "SELECT person_id FROM cairn_auth_sessions
             WHERE token_hash = $1 AND revoked_at IS NULL",
        )
        .bind(&token_hash)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| PostgresError::Validation("account authentication failed".to_owned()))?;
        let result = sqlx::query(
            "UPDATE cairn_auth_sessions SET revoked_at = now()
             WHERE token_hash = $1 AND revoked_at IS NULL",
        )
        .bind(token_hash)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(PostgresError::Validation(
                "account authentication failed".to_owned(),
            ));
        }
        self.record_audit_event(
            &person_id,
            None,
            "account_logout",
            Some(&person_id),
            serde_json::json!({}),
        )
        .await?;
        Ok(())
    }

    async fn issue_session(&self, account: Account) -> Result<AuthSessionResult, PostgresError> {
        let token = random_token("cairn-session");
        let token_hash = cairn_core::hash_token(&token);
        let session_id = format!("auth-{}", &token_hash[..16]);
        let expires_at = now_seconds() + AUTH_SESSION_SECONDS;
        let mut transaction = self.pool.begin().await?;
        insert_auth_session(
            &mut transaction,
            &session_id,
            &account.person_id,
            &token_hash,
            expires_at,
        )
        .await?;
        record_audit(
            &mut transaction,
            &account.person_id,
            None,
            "account_login",
            Some(&account.person_id),
            serde_json::json!({}),
        )
        .await?;
        transaction.commit().await?;
        Ok(AuthSessionResult {
            account,
            token,
            expires_at: expires_at.to_string(),
        })
    }

    pub async fn start_session(
        &self,
        account_token: &str,
        session_id: &str,
        context: WorkContext,
    ) -> Result<Session, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        if session_id.trim().is_empty() {
            return Err(PostgresError::Validation(
                "session_id must not be empty".to_owned(),
            ));
        }
        let context = redact_work_context(&context);
        self.finish_idle_sessions(&account.person_id).await;
        let context = serde_json::to_value(&context)?;
        let empty_context = serde_json::to_value(WorkContext::default())?;
        let mut transaction = self.pool.begin().await?;
        // `canonical || stored = canonical` counts omitted default fields as empty.
        // An extra key or a real value keeps the row. PostgreSQL rechecks this
        // after waiting, so the first non-empty context stays.
        sqlx::query(
            "INSERT INTO cairn_sessions (session_id, person_id, context)
             VALUES ($1, $2, $3)
             ON CONFLICT (person_id, session_id) DO UPDATE
             SET context = EXCLUDED.context
             WHERE cairn_sessions.person_id = $2
               AND cairn_sessions.session_id = $1
               AND ($4::jsonb || cairn_sessions.context) = $4::jsonb
               AND EXCLUDED.context <> $4::jsonb",
        )
        .bind(session_id)
        .bind(&account.person_id)
        .bind(&context)
        .bind(&empty_context)
        .execute(&mut *transaction)
        .await?;
        let row = sqlx::query(
            "SELECT session_id,
                    EXTRACT(EPOCH FROM started_at)::bigint::text AS started_at,
                    EXTRACT(EPOCH FROM ended_at)::bigint::text AS ended_at,
                    context
             FROM cairn_sessions WHERE person_id = $1 AND session_id = $2",
        )
        .bind(&account.person_id)
        .bind(session_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        session_from_row(&row)
    }

    pub async fn remember_with_kind(
        &self,
        account_token: &str,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
        lifecycle: cairn_core::LifecycleInput,
        kind: MemoryKind,
    ) -> Result<Memory, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        let memory = build_explicit_memory_record(content, concepts, context, lifecycle, kind)?;
        let mut transaction = self.pool.begin().await?;
        insert_memory(&mut transaction, &account.person_id, &memory).await?;
        transaction.commit().await?;
        Ok(memory)
    }

    pub async fn end_session(
        &self,
        account_token: &str,
        session_id: &str,
    ) -> Result<Memory, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        self.finish_session(&account.person_id, session_id).await
    }

    /// Explicit end. A summary the person forgot or corrected is reported, not returned.
    async fn finish_session(
        &self,
        person_id: &str,
        session_id: &str,
    ) -> Result<Memory, PostgresError> {
        self.finish_session_summary(person_id, session_id)
            .await?
            .ok_or_else(|| {
                PostgresError::Validation(format!(
                    "session summary was forgotten or superseded: {}",
                    session_memory_id(Some(person_id), session_id)
                ))
            })
    }

    /// Ends the session and derives its summary. `None`: the person forgot or corrected the
    /// summary, so it stays as it is and is not derived again under the same id.
    async fn finish_session_summary(
        &self,
        person_id: &str,
        session_id: &str,
    ) -> Result<Option<Memory>, PostgresError> {
        if session_id.trim().is_empty() {
            return Err(PostgresError::Validation(
                "session_id must not be empty".to_owned(),
            ));
        }
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "UPDATE cairn_sessions SET ended_at = COALESCE(ended_at, now())
             WHERE person_id = $1 AND session_id = $2",
        )
        .bind(person_id)
        .bind(session_id)
        .execute(&mut *transaction)
        .await?;
        let session_row = sqlx::query(
            "SELECT session_id,
                    EXTRACT(EPOCH FROM started_at)::bigint::text AS started_at,
                    EXTRACT(EPOCH FROM ended_at)::bigint::text AS ended_at,
                    context
             FROM cairn_sessions WHERE person_id = $1 AND session_id = $2",
        )
        .bind(person_id)
        .bind(session_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| PostgresError::Validation(format!("unknown session: {session_id}")))?;
        let session = session_from_row(&session_row)?;
        let memory_id = session_memory_id(Some(person_id), session_id);
        let observation_query = format!(
            "{} WHERE o.person_id = $1 AND o.session_id = $2
               AND NOT EXISTS (
                   SELECT 1 FROM cairn_forgotten_items f
                   WHERE f.person_id = o.person_id AND f.item_id = o.event_id
               )
               AND (o.lifecycle->>'expiresAt' IS NULL OR
                    (o.lifecycle->>'expiresAt')::bigint > EXTRACT(EPOCH FROM now())::bigint)
             ORDER BY o.captured_at",
            observation_select_sql()
        );
        let observation_rows = sqlx::query(&observation_query)
            .bind(person_id)
            .bind(session_id)
            .fetch_all(&mut *transaction)
            .await?;
        let mut observations = observation_rows
            .iter()
            .map(observation_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let existing = sqlx::query(&session_memory_select_sql())
            .bind(person_id)
            .bind(&memory_id)
            .fetch_optional(&mut *transaction)
            .await?;
        let mut earlier = None;
        if let Some(row) = existing {
            let memory = memory_from_row(&row)?;
            let hidden = sqlx::query_scalar::<_, bool>(&format!(
                "SELECT EXISTS (SELECT 1 FROM cairn_forgotten_items f
                                WHERE f.person_id = $1 AND f.item_id = $2)
                     OR EXISTS (SELECT 1 FROM cairn_memories m
                                WHERE m.person_id = $1 AND m.memory_id = $2 AND NOT {})",
                not_superseded_by_live_memory_sql(false)
            ))
            .bind(person_id)
            .bind(&memory_id)
            .fetch_one(&mut *transaction)
            .await?;
            if hidden {
                clear_session_pending(&mut transaction, person_id, session_id).await?;
                transaction.commit().await?;
                return Ok(None);
            }
            let forgotten_source = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM cairn_forgotten_items f
                                WHERE f.person_id = $1 AND f.item_id = ANY($2))",
            )
            .bind(person_id)
            .bind(&memory.source_event_ids)
            .fetch_one(&mut *transaction)
            .await?;
            if !forgotten_source
                && !observations
                    .iter()
                    .any(|observation| !memory.source_event_ids.contains(&observation.event_id))
            {
                clear_session_pending(&mut transaction, person_id, session_id).await?;
                transaction.commit().await?;
                return Ok(Some(memory));
            }
            // The session id was reused after its summary, or a source was forgotten: derive
            // the summary again from what is retained.
            sqlx::query("DELETE FROM cairn_memories WHERE person_id = $1 AND memory_id = $2")
                .bind(person_id)
                .bind(&memory_id)
                .execute(&mut *transaction)
                .await?;
            earlier = Some(memory);
        }
        // Summary input only. Stored observations and sync payloads stay unchanged.
        for observation in &mut observations {
            if observation.context == WorkContext::default() {
                observation.context.clone_from(&session.context);
            }
        }
        let mut memory =
            derive_session_memory(Some(person_id), session_id, &session.context, &observations)?;
        if let Some(earlier) = &earlier {
            carry_summary_state(earlier, &mut memory);
        }
        insert_memory(&mut transaction, person_id, &memory).await?;
        clear_session_pending(&mut transaction, person_id, session_id).await?;
        transaction.commit().await?;
        Ok(Some(memory))
    }

    /// Finishes this person's sessions that have been pending (a capture the summary may not
    /// cover) for longer than `IDLE_SESSION_SECONDS`, at most `MAX_SWEEP_SESSIONS` per call.
    /// Reads only the partial pending index. A session that fails is marked pending again, so
    /// it is retried after another idle period, not on every push. Failures are logged; the
    /// capture or session start that called this has already succeeded.
    async fn finish_idle_sessions(&self, person_id: &str) {
        let sessions = sqlx::query_scalar::<_, String>(
            "SELECT session_id FROM cairn_sessions
             WHERE person_id = $1 AND pending_since IS NOT NULL
               AND pending_since < now() - make_interval(secs => $2)
             ORDER BY pending_since LIMIT $3",
        )
        .bind(person_id)
        .bind(IDLE_SESSION_SECONDS as f64)
        .bind(MAX_SWEEP_SESSIONS)
        .fetch_all(&self.pool)
        .await;
        match sessions {
            Ok(sessions) => {
                for session_id in sessions {
                    self.finish_idle_session(person_id, &session_id).await;
                }
            }
            Err(error) => eprintln!("cairn: automatic session close failed: {error}"),
        }
    }

    /// Periodic close: the idle sweep for every person, at most `MAX_SWEEP_SESSIONS` sessions
    /// per call, oldest pending first, from the same partial pending index. Failures are
    /// logged and never returned; a failed session backs off like `finish_idle_sessions`.
    pub async fn sweep_idle_sessions(&self) {
        let sessions = sqlx::query_as::<_, (String, String)>(
            "SELECT person_id, session_id FROM cairn_sessions
             WHERE pending_since IS NOT NULL
               AND pending_since < now() - make_interval(secs => $1)
             ORDER BY pending_since LIMIT $2",
        )
        .bind(IDLE_SESSION_SECONDS as f64)
        .bind(MAX_SWEEP_SESSIONS)
        .fetch_all(&self.pool)
        .await;
        match sessions {
            Ok(sessions) => {
                for (person_id, session_id) in sessions {
                    self.finish_idle_session(&person_id, &session_id).await;
                }
            }
            Err(error) => eprintln!("cairn: automatic session close failed: {error}"),
        }
    }

    async fn finish_idle_session(&self, person_id: &str, session_id: &str) {
        // A summary the person hid comes back as `None` and is left as it is.
        if let Err(error) = self.finish_session_summary(person_id, session_id).await {
            eprintln!("cairn: automatic session close failed: {error}");
            let backoff = sqlx::query(
                "UPDATE cairn_sessions SET pending_since = now()
                 WHERE person_id = $1 AND session_id = $2",
            )
            .bind(person_id)
            .bind(session_id)
            .execute(&self.pool)
            .await;
            if let Err(error) = backoff {
                eprintln!("cairn: automatic session close failed: {error}");
            }
        }
    }

    /// Automatic registration on capture. The person's own project for the repository comes
    /// first, then one they are a member of; only when there is none, a project owned by this
    /// person is created. A branch is registered on a project the person owns (branch writes
    /// are owner-only); an empty or detached branch is skipped. Idempotent.
    async fn auto_register_project(
        &self,
        person_id: &str,
        context: &WorkContext,
    ) -> Result<(), PostgresError> {
        let Some(repository) = context
            .repository
            .as_deref()
            .map(str::trim)
            .filter(|repository| !repository.is_empty())
        else {
            return Ok(());
        };
        validate_project_text(repository, "repository")?;
        let branch = context
            .branch
            .as_deref()
            .map(str::trim)
            .filter(|branch| !branch.is_empty() && *branch != "HEAD");
        if let Some(branch) = branch {
            validate_project_text(branch, "branch name")?;
        }
        let mut transaction = self.pool.begin().await?;
        let owned = sqlx::query_scalar::<_, String>(
            "SELECT project_id FROM cairn_projects WHERE person_id = $1 AND repository = $2",
        )
        .bind(person_id)
        .bind(repository)
        .fetch_optional(&mut *transaction)
        .await?;
        let project_id = match owned {
            Some(project_id) => project_id,
            None => {
                let is_member = sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS (
                        SELECT 1 FROM cairn_projects p
                        JOIN cairn_project_members m
                          ON m.owner_person_id = p.person_id AND m.project_id = p.project_id
                        WHERE m.member_person_id = $1 AND p.repository = $2)",
                )
                .bind(person_id)
                .bind(repository)
                .fetch_one(&mut *transaction)
                .await?;
                if is_member {
                    return Ok(());
                }
                let project_id = format!(
                    "project-{}",
                    &cairn_core::hash_token(&random_token("project"))[..24]
                );
                sqlx::query(
                    "INSERT INTO cairn_projects (person_id, project_id, repository, name)
                     VALUES ($1, $2, $3, $3) ON CONFLICT (person_id, repository) DO NOTHING",
                )
                .bind(person_id)
                .bind(&project_id)
                .bind(repository)
                .execute(&mut *transaction)
                .await?;
                sqlx::query_scalar::<_, String>(
                    "SELECT project_id FROM cairn_projects
                     WHERE person_id = $1 AND repository = $2",
                )
                .bind(person_id)
                .bind(repository)
                .fetch_one(&mut *transaction)
                .await?
            }
        };
        if let Some(branch) = branch {
            let branch_id = format!(
                "branch-{}",
                &cairn_core::hash_token(&random_token("branch"))[..24]
            );
            sqlx::query(
                "INSERT INTO cairn_project_branches (person_id, branch_id, project_id, name)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (person_id, project_id, name) DO NOTHING",
            )
            .bind(person_id)
            .bind(&branch_id)
            .bind(&project_id)
            .bind(branch)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn forget(&self, account_token: &str, item_id: &str) -> Result<(), PostgresError> {
        self.hide_owned_item(account_token, item_id, "memory_forget")
            .await
    }

    pub async fn delete_memory(
        &self,
        account_token: &str,
        item_id: &str,
    ) -> Result<(), PostgresError> {
        self.hide_owned_item(account_token, item_id, "memory_delete_retained")
            .await
    }

    async fn hide_owned_item(
        &self,
        account_token: &str,
        item_id: &str,
        action: &str,
    ) -> Result<(), PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        if item_id.trim().is_empty() {
            return Err(PostgresError::Validation("id must not be empty".to_owned()));
        }
        let mut transaction = self.pool.begin().await?;
        let memory = sqlx::query(
            "SELECT source_event_ids, promoted_from FROM cairn_memories
             WHERE person_id = $1 AND memory_id = $2",
        )
        .bind(&account.person_id)
        .bind(item_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let observation_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM cairn_observations
             WHERE person_id = $1 AND event_id = $2)",
        )
        .bind(&account.person_id)
        .bind(item_id)
        .fetch_one(&mut *transaction)
        .await?;
        if memory.is_none() && !observation_exists {
            let shared = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (
                    SELECT 1 FROM cairn_project_shares
                    WHERE member_person_id = $1 AND memory_id = $2
                 )",
            )
            .bind(&account.person_id)
            .bind(item_id)
            .fetch_one(&mut *transaction)
            .await?;
            transaction.rollback().await?;
            if shared {
                return Err(PostgresError::Validation(
                    "account does not own the memory".to_owned(),
                ));
            }
            return Err(PostgresError::Validation(format!(
                "unknown memory or event: {item_id}"
            )));
        }
        insert_forget(&mut transaction, &account.person_id, item_id).await?;
        // A summary built from a forgotten observation is derived again without it.
        sqlx::query(
            "UPDATE cairn_sessions SET pending_since = now()
             WHERE person_id = $1 AND session_id IN
                   (SELECT session_id FROM cairn_observations
                    WHERE person_id = $1 AND event_id = $2)",
        )
        .bind(&account.person_id)
        .bind(item_id)
        .execute(&mut *transaction)
        .await?;
        // Session row first, then the cursor lock: the order sync_push takes them in.
        insert_sync_deletion(&mut transaction, &account.person_id, item_id).await?;
        // A promoted copy never cascades: its origin and the origin's events stay personal.
        if let Some(memory) = memory
            && memory
                .try_get::<Option<String>, _>("promoted_from")?
                .is_none()
        {
            let source_event_ids: Vec<String> = memory
                .try_get::<Json<Vec<String>>, _>("source_event_ids")?
                .0;
            for event_id in source_event_ids {
                insert_forget(&mut transaction, &account.person_id, &event_id).await?;
                insert_sync_deletion(&mut transaction, &account.person_id, &event_id).await?;
            }
        }
        record_audit(
            &mut transaction,
            &account.person_id,
            None,
            action,
            Some(item_id),
            serde_json::json!({}),
        )
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn search(
        &self,
        account_token: &str,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<Vec<RecallResult>, PostgresError> {
        Ok(self
            .search_window(account_token, query, limit, filter)
            .await?
            .0)
    }

    /// Returns the capped result window and the number of eligible matches before the cap.
    ///
    /// The total comes from `COUNT(*) OVER ()` in the same statement, so it shares the
    /// snapshot and predicate of the rows. It is read from the returned rows, so callers
    /// that need it must ask for at least one row (`context` passes `usize::MAX`;
    /// `why_changed_context` has its own query).
    async fn search_window(
        &self,
        account_token: &str,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<(Vec<RecallResult>, usize), PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        if query.trim().is_empty() {
            return Err(PostgresError::Validation(
                "query must not be empty".to_owned(),
            ));
        }
        let limit = i64::try_from(limit.min(MAX_SEARCH_RESULTS as usize))
            .map_err(|_| PostgresError::Validation("limit exceeds PostgreSQL range".to_owned()))?;
        let (conditions, binds) = search_conditions(&account.person_id, query, filter);
        let limit_bind = binds.len() + 1;
        let sql = format!(
            "{} WHERE {} ORDER BY ts_rank_cd(m.search_tsv, plainto_tsquery('simple', $2)) DESC,
             (m.kind = 'decision') DESC, m.memory_id ASC LIMIT ${limit_bind}",
            memory_search_select_sql(),
            conditions.join(" AND ")
        );
        let mut request = sqlx::query(&sql);
        for value in &binds {
            request = request.bind(value);
        }
        request = request.bind(limit);
        let rows = request.fetch_all(&self.pool).await?;
        let total = match rows.first() {
            Some(row) => usize::try_from(row.try_get::<i64, _>("total_matches")?)
                .map_err(|_| PostgresError::Validation("match count is out of range".to_owned()))?,
            None => 0,
        };
        let all_memories = self.all_memories_for_person(&account.person_id).await?;
        let mut results = rows
            .iter()
            .map(|row| {
                let memory = memory_from_row(row)?;
                let score: f32 = row.try_get("search_score")?;
                Ok(recall_result(
                    &memory,
                    score,
                    &all_memories,
                    &HashSet::new(),
                    filter,
                ))
            })
            .collect::<Result<Vec<_>, PostgresError>>()?;
        // Applies inside the SQL window; a tie cut by the row cap stays in memory_id order.
        prefer_newer_contradiction(&mut results, &all_memories);
        Ok((results, total))
    }

    pub async fn context(
        &self,
        account_token: &str,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, PostgresError> {
        let (mut results, total) = self
            .search_window(account_token, query, usize::MAX, filter)
            .await?;
        Ok(pack_window(query.to_owned(), &mut results, limit, total))
    }

    pub async fn resume_context(
        &self,
        account_token: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        let memories = self.all_memories_for_person(&account.person_id).await?;
        let mut results = memories
            .iter()
            .filter(|memory| memory.kind == MemoryKind::SessionSummary)
            .filter(|memory| memory_matches_filter(memory, filter))
            .map(|memory| recall_result(memory, 1.0, &memories, &HashSet::new(), filter))
            .collect::<Vec<_>>();
        Ok(build_context_pack("resume".to_owned(), &mut results, limit))
    }

    pub async fn unresolved_context(
        &self,
        account_token: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        let memories = self.all_memories_for_person(&account.person_id).await?;
        let mut results = memories
            .iter()
            .filter(|memory| memory.kind == MemoryKind::SessionSummary)
            .filter(|memory| !memory.unresolved_work.is_empty())
            .filter(|memory| memory_matches_filter(memory, filter))
            .map(|memory| recall_result(memory, 1.0, &memories, &HashSet::new(), filter))
            .collect::<Vec<_>>();
        Ok(build_context_pack(
            "unresolved".to_owned(),
            &mut results,
            limit,
        ))
    }

    /// Ranks decision evidence across every eligible match, then applies the 512 cap.
    ///
    /// One uncapped query returns only `(memory_id, score)` for each eligible match, in the
    /// same lexical order as `search`. Each id is resolved in the person's live memories
    /// (the set recall metadata already loads), ranked with `why_changed_priority`, stably
    /// sorted by decision, then relationship count, then that lexical order, and cut to
    /// `MAX_SEARCH_RESULTS` before full results are built. Cost grows with the number of
    /// matches, not with 512: every match is scored and inspected in Rust.
    pub async fn why_changed_context(
        &self,
        account_token: &str,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        if query.trim().is_empty() {
            return Err(PostgresError::Validation(
                "query must not be empty".to_owned(),
            ));
        }
        let (conditions, binds) = search_conditions(&account.person_id, query, filter);
        let sql = format!(
            "SELECT m.memory_id,
                    ts_rank_cd(m.search_tsv, plainto_tsquery('simple', $2)) AS search_score
             FROM cairn_memories m WHERE {}
             ORDER BY ts_rank_cd(m.search_tsv, plainto_tsquery('simple', $2)) DESC,
             m.memory_id ASC",
            conditions.join(" AND ")
        );
        let mut request = sqlx::query(&sql);
        for value in &binds {
            request = request.bind(value);
        }
        let rows = request.fetch_all(&self.pool).await?;
        let all_memories = self.all_memories_for_person(&account.person_id).await?;
        let by_id: HashMap<&str, &Memory> = all_memories
            .iter()
            .map(|memory| (memory.memory_id.as_str(), memory))
            .collect();
        // A match deleted between the two reads has no memory to rank and is not counted.
        let mut ranked = Vec::with_capacity(rows.len());
        for row in &rows {
            let id: String = row.try_get("memory_id")?;
            if let Some(memory) = by_id.get(id.as_str()) {
                ranked.push((*memory, row.try_get::<f32, _>("search_score")?));
            }
        }
        let total = ranked.len();
        // Stable: equal priority keeps the lexical order from the query.
        ranked.sort_by_cached_key(|(memory, _)| {
            let (decision, relationships) = why_changed_priority(memory);
            (
                std::cmp::Reverse(decision),
                std::cmp::Reverse(relationships),
            )
        });
        let mut results = ranked
            .into_iter()
            .take(MAX_SEARCH_RESULTS as usize)
            .map(|(memory, score)| {
                recall_result(memory, score, &all_memories, &HashSet::new(), filter)
            })
            .collect::<Vec<_>>();
        Ok(pack_window(
            format!("why changed: {query}"),
            &mut results,
            limit,
            total,
        ))
    }

    pub async fn correct(
        &self,
        account_token: &str,
        target_id: &str,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
        lifecycle: LifecycleInput,
    ) -> Result<Memory, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        if target_id.trim().is_empty() {
            return Err(PostgresError::Validation(
                "target_id must not be empty".to_owned(),
            ));
        }
        let mut replacement = build_explicit_memory_record(
            content,
            concepts,
            context.clone(),
            lifecycle,
            MemoryKind::Correction,
        )?;
        let mut transaction = self.pool.begin().await?;
        let target_memory = sqlx::query(&format!(
            "{} WHERE m.person_id = $1 AND m.memory_id = $2",
            memory_select_sql()
        ))
        .bind(&account.person_id)
        .bind(target_id)
        .fetch_optional(&mut *transaction)
        .await?
        .map(|row| memory_from_row(&row))
        .transpose()?;
        let target_observation = sqlx::query(&format!(
            "{} WHERE o.person_id = $1 AND o.event_id = $2",
            observation_select_sql()
        ))
        .bind(&account.person_id)
        .bind(target_id)
        .fetch_optional(&mut *transaction)
        .await?
        .map(|row| observation_from_row(&row))
        .transpose()?;
        let (target_context, target_scope, target_project_id) = if let Some(memory) = target_memory
        {
            (memory.context, memory.scope, memory.project_id)
        } else if let Some(observation) = target_observation {
            (observation.context, observation.scope, None)
        } else {
            let shared = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (
                    SELECT 1 FROM cairn_project_shares
                    WHERE member_person_id = $1 AND memory_id = $2
                 )",
            )
            .bind(&account.person_id)
            .bind(target_id)
            .fetch_one(&mut *transaction)
            .await?;
            if shared {
                return Err(PostgresError::Validation(
                    "account does not own the memory".to_owned(),
                ));
            }
            return Err(PostgresError::Validation(format!(
                "unknown memory or event: {target_id}"
            )));
        };
        if context == WorkContext::default() {
            replacement.lifecycle.sensitivity =
                sensitivity_including_context(&replacement.lifecycle.sensitivity, &target_context);
            replacement.context = redact_work_context(&target_context);
            replacement.scope = target_scope;
        }
        replacement.lifecycle.confidence = 100;
        replacement.project_id = target_project_id;
        replacement.supersedes = Some(target_id.to_owned());
        insert_memory(&mut transaction, &account.person_id, &replacement).await?;
        record_audit(
            &mut transaction,
            &account.person_id,
            None,
            "memory_correct",
            Some(target_id),
            serde_json::json!({ "replacement_id": replacement.memory_id }),
        )
        .await?;
        transaction.commit().await?;
        Ok(replacement)
    }

    pub async fn register_project(
        &self,
        account_token: &str,
        repository: &str,
        name: &str,
        default_branch: Option<String>,
    ) -> Result<Project, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        let repository = repository.trim();
        let name = name.trim();
        if repository.is_empty() || name.is_empty() {
            return Err(PostgresError::Validation(
                "repository and project name must not be empty".to_owned(),
            ));
        }
        let existing = sqlx::query(
            "SELECT project_id, repository, name, default_branch,
                    EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at,
                    EXTRACT(EPOCH FROM updated_at)::bigint::text AS updated_at
             FROM cairn_projects WHERE person_id = $1 AND repository = $2",
        )
        .bind(&account.person_id)
        .bind(repository)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(row) = existing {
            return project_from_row(&row);
        }
        let project_id = format!(
            "project-{}",
            &cairn_core::hash_token(&random_token("project"))[..24]
        );
        let row = sqlx::query(
            "INSERT INTO cairn_projects (person_id, project_id, repository, name, default_branch)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING project_id, repository, name, default_branch,
                       EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at,
                       EXTRACT(EPOCH FROM updated_at)::bigint::text AS updated_at",
        )
        .bind(&account.person_id)
        .bind(&project_id)
        .bind(repository)
        .bind(name)
        .bind(default_branch)
        .fetch_one(&self.pool)
        .await?;
        project_from_row(&row)
    }

    pub async fn projects(&self, account_token: &str) -> Result<Vec<Project>, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        let rows = sqlx::query(
            "SELECT project_id, repository, name, default_branch,
                    EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at,
                    EXTRACT(EPOCH FROM updated_at)::bigint::text AS updated_at
             FROM cairn_projects
             WHERE person_id = $1
                OR project_id IN (
                    SELECT project_id FROM cairn_project_members WHERE member_person_id = $1
                )
             ORDER BY repository",
        )
        .bind(&account.person_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(project_from_row).collect()
    }

    pub async fn register_branch(
        &self,
        account_token: &str,
        project_id: &str,
        name: &str,
        base_branch: Option<String>,
        base_commit: Option<String>,
        head_commit: Option<String>,
    ) -> Result<ProjectBranch, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        validate_project_text(project_id, "project_id")?;
        validate_project_text(name, "branch name")?;
        self.require_own_project(&account.person_id, project_id)
            .await?;
        let existing = sqlx::query(&branch_select_sql("b.project_id = $2 AND b.name = $3"))
            .bind(&account.person_id)
            .bind(project_id)
            .bind(name.trim())
            .fetch_optional(&self.pool)
            .await?;
        if let Some(row) = existing {
            return project_branch_from_row(&row);
        }
        let branch_id = format!(
            "branch-{}",
            &cairn_core::hash_token(&random_token("branch"))[..24]
        );
        let row = sqlx::query(
            "INSERT INTO cairn_project_branches
                (person_id, branch_id, project_id, name, base_branch, base_commit, head_commit)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             RETURNING branch_id, project_id, name, base_branch, base_commit, head_commit,
                       status, status_reason, merge_commit, merge_memory_id,
                       EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at,
                       EXTRACT(EPOCH FROM updated_at)::bigint::text AS updated_at",
        )
        .bind(&account.person_id)
        .bind(&branch_id)
        .bind(project_id)
        .bind(name.trim())
        .bind(base_branch)
        .bind(base_commit)
        .bind(head_commit)
        .fetch_one(&self.pool)
        .await?;
        project_branch_from_row(&row)
    }

    pub async fn branches(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectBranch>, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        validate_project_text(project_id, "project_id")?;
        let rows = sqlx::query(&branch_select_sql("b.project_id = $2"))
            .bind(&account.person_id)
            .bind(project_id)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(project_branch_from_row).collect()
    }

    pub async fn register_worktree(
        &self,
        account_token: &str,
        project_id: &str,
        branch_id: Option<String>,
        path: &str,
        commit: Option<String>,
    ) -> Result<ProjectWorktree, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        validate_project_text(project_id, "project_id")?;
        validate_project_text(path, "worktree path")?;
        self.require_own_project(&account.person_id, project_id)
            .await?;
        if let Some(branch_id) = &branch_id {
            let branch_exists = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (
                    SELECT 1 FROM cairn_project_branches
                    WHERE person_id = $1 AND project_id = $2 AND branch_id = $3
                )",
            )
            .bind(&account.person_id)
            .bind(project_id)
            .bind(branch_id)
            .fetch_one(&self.pool)
            .await?;
            if !branch_exists {
                return Err(PostgresError::Validation(format!(
                    "unknown project branch: {branch_id}"
                )));
            }
        }
        let existing = sqlx::query(&worktree_select_sql("w.project_id = $2 AND w.path = $3"))
            .bind(&account.person_id)
            .bind(project_id)
            .bind(path.trim())
            .fetch_optional(&self.pool)
            .await?;
        if let Some(row) = existing {
            return project_worktree_from_row(&row);
        }
        let worktree_id = format!(
            "worktree-{}",
            &cairn_core::hash_token(&random_token("worktree"))[..24]
        );
        let row = sqlx::query(
            "INSERT INTO cairn_project_worktrees
                (person_id, worktree_id, project_id, branch_id, path, commit_sha)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING worktree_id, project_id, branch_id, path, commit_sha, status,
                       EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at,
                       EXTRACT(EPOCH FROM updated_at)::bigint::text AS updated_at",
        )
        .bind(&account.person_id)
        .bind(&worktree_id)
        .bind(project_id)
        .bind(branch_id)
        .bind(path.trim())
        .bind(commit)
        .fetch_one(&self.pool)
        .await?;
        project_worktree_from_row(&row)
    }

    pub async fn worktrees(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectWorktree>, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        validate_project_text(project_id, "project_id")?;
        let rows = sqlx::query(&worktree_select_sql("w.project_id = $2"))
            .bind(&account.person_id)
            .bind(project_id)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(project_worktree_from_row).collect()
    }

    pub async fn register_project_entity(
        &self,
        account_token: &str,
        project_id: &str,
        input: ProjectEntityInput,
    ) -> Result<ProjectEntity, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        validate_project_text(project_id, "project_id")?;
        validate_project_text(&input.key, "entity key")?;
        validate_entity_kind(&input.kind)?;
        let project_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM cairn_projects
             WHERE person_id = $1 AND project_id = $2)",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .fetch_one(&self.pool)
        .await?;
        if !project_exists {
            return Err(PostgresError::Validation(format!(
                "unknown project: {project_id}"
            )));
        }
        if let Some(memory_id) = &input.source_memory_id {
            let source = sqlx::query(
                "SELECT context FROM cairn_memories
                 WHERE person_id = $1 AND memory_id = $2",
            )
            .bind(&account.person_id)
            .bind(memory_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| PostgresError::Validation(format!("unknown memory: {memory_id}")))?;
            let context: WorkContext = source.try_get::<Json<WorkContext>, _>("context")?.0;
            let repository = sqlx::query_scalar::<_, String>(
                "SELECT repository FROM cairn_projects
                 WHERE person_id = $1 AND project_id = $2",
            )
            .bind(&account.person_id)
            .bind(project_id)
            .fetch_one(&self.pool)
            .await?;
            if context.repository.as_deref() != Some(repository.as_str()) {
                return Err(PostgresError::Validation(
                    "entity source repository does not match the project".to_owned(),
                ));
            }
        }
        let content = input
            .content
            .map(|value| redact_sensitive_content(value.trim()))
            .filter(|value| !value.is_empty());
        if content.as_ref().is_some_and(|value| value.len() > 2048) {
            return Err(PostgresError::Validation(
                "entity content exceeds the 2048 byte limit".to_owned(),
            ));
        }
        let commit = input
            .commit
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if commit.as_ref().is_some_and(|value| value.len() > 256) {
            return Err(PostgresError::Validation(
                "entity commit exceeds the 256 byte limit".to_owned(),
            ));
        }
        let entity_id = format!(
            "entity-{}",
            &cairn_core::hash_token(&random_token("entity"))[..24]
        );
        let row = sqlx::query(
            "INSERT INTO cairn_project_entities
                (person_id, entity_id, project_id, kind, entity_key, content,
                 source_memory_id, commit_sha)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (person_id, project_id, kind, entity_key)
             DO UPDATE SET updated_at = now()
             RETURNING entity_id, project_id, kind, entity_key, content,
                       source_memory_id, commit_sha, status,
                       EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at,
                       EXTRACT(EPOCH FROM updated_at)::bigint::text AS updated_at",
        )
        .bind(&account.person_id)
        .bind(&entity_id)
        .bind(project_id)
        .bind(input.kind.trim())
        .bind(input.key.trim())
        .bind(content)
        .bind(input.source_memory_id)
        .bind(commit)
        .fetch_one(&self.pool)
        .await?;
        project_entity_from_row(&row)
    }

    pub async fn project_entities(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectEntity>, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        validate_project_text(project_id, "project_id")?;
        let rows = sqlx::query(
            "SELECT entity_id, project_id, kind, entity_key, content,
                    source_memory_id, commit_sha, status,
                    EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at,
                    EXTRACT(EPOCH FROM updated_at)::bigint::text AS updated_at
             FROM cairn_project_entities
             WHERE person_id = $1 AND project_id = $2 AND status = 'active'
             ORDER BY kind, entity_key",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(project_entity_from_row).collect()
    }

    pub async fn freshness_history(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectFreshnessEvent>, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        validate_project_text(project_id, "project_id")?;
        let rows = sqlx::query(
            "SELECT event_id, project_id, memory_id, previous_commit, commit_sha,
                    action, reason,
                    EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at
             FROM cairn_project_freshness_events
             WHERE person_id = $1 AND project_id = $2
             ORDER BY created_at DESC, event_id",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(project_freshness_event_from_row).collect()
    }

    pub async fn project_maintenance(
        &self,
        account_token: &str,
        project_id: &str,
        current_commit: Option<&str>,
    ) -> Result<ProjectMaintenanceReport, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        validate_project_text(project_id, "project_id")?;
        let query = format!(
            "{} WHERE m.person_id = $1 AND m.project_id = $2
              AND NOT EXISTS (SELECT 1 FROM cairn_forgotten_items f
                              WHERE f.person_id = m.person_id AND f.item_id = m.memory_id)
              ORDER BY m.created_at DESC, m.memory_id",
            memory_select_sql()
        );
        let rows = sqlx::query(&query)
            .bind(&account.person_id)
            .bind(project_id)
            .fetch_all(&self.pool)
            .await?;
        let memories = rows
            .iter()
            .map(memory_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let superseded_memory_ids = memories
            .iter()
            .filter(|memory| {
                memories.iter().any(|candidate| {
                    candidate.supersedes.as_deref() == Some(memory.memory_id.as_str())
                })
            })
            .map(|memory| memory.memory_id.clone())
            .collect::<Vec<_>>();
        let stale_memory_ids = current_commit
            .map(|commit| {
                memories
                    .iter()
                    .filter(|memory| {
                        memory
                            .verified_commit
                            .as_deref()
                            .is_some_and(|verified| verified != commit)
                    })
                    .map(|memory| memory.memory_id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let active_memory_ids = memories
            .iter()
            .filter(|memory| !superseded_memory_ids.contains(&memory.memory_id))
            .map(|memory| memory.memory_id.clone())
            .collect();
        Ok(ProjectMaintenanceReport {
            project_id: project_id.to_owned(),
            current_commit: current_commit.map(str::to_owned),
            stale_memory_ids,
            superseded_memory_ids,
            active_memory_ids,
            checked_at: now_seconds().to_string(),
        })
    }

    pub async fn confirm_branch_merge(
        &self,
        account_token: &str,
        project_id: &str,
        branch_id: &str,
        memory_id: &str,
        commit: &str,
        reason: &str,
    ) -> Result<ProjectMergeResult, PostgresError> {
        let (account, via_device) = self.authenticate_principal(account_token).await?;
        validate_project_text(project_id, "project_id")?;
        validate_project_text(branch_id, "branch_id")?;
        validate_project_text(memory_id, "memory_id")?;
        let commit = commit.trim();
        let reason = reason.trim();
        if commit.is_empty() || reason.is_empty() {
            return Err(PostgresError::Validation(
                "merge commit and confirmation reason are required".to_owned(),
            ));
        }
        if commit.len() > 256 {
            return Err(PostgresError::Validation(
                "commit exceeds the 256 byte limit".to_owned(),
            ));
        }
        if reason.len() > 2048 {
            return Err(PostgresError::Validation(
                "merge reason exceeds the 2048 byte limit".to_owned(),
            ));
        }

        let mut transaction = self.pool.begin().await?;
        let project = sqlx::query(
            "SELECT project_id, repository, name, default_branch,
                    EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at,
                    EXTRACT(EPOCH FROM updated_at)::bigint::text AS updated_at
             FROM cairn_projects WHERE person_id = $1 AND project_id = $2
             FOR UPDATE",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| PostgresError::Validation(format!("unknown project: {project_id}")))?;
        let project = project_from_row(&project)?;
        // The row lock conflicts with the key-share lock a concurrent member insert takes through
        // its foreign key, so the member check below cannot race an invite.
        if via_device
            && sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM cairn_project_members
                                WHERE owner_person_id = $1 AND project_id = $2
                                  AND member_person_id <> $1)
                     OR EXISTS (SELECT 1 FROM cairn_project_team_members
                                WHERE owner_person_id = $1 AND project_id = $2
                                  AND member_person_id <> $1)",
            )
            .bind(&account.person_id)
            .bind(project_id)
            .fetch_one(&mut *transaction)
            .await?
        {
            return Err(PostgresError::Validation(
                cairn_core::WEB_ONLY_ERROR.to_owned(),
            ));
        }
        let branch_query = format!(
            "{} FOR UPDATE",
            branch_select_sql("b.project_id = $2 AND b.branch_id = $3")
        );
        let branch_row = sqlx::query(&branch_query)
            .bind(&account.person_id)
            .bind(project_id)
            .bind(branch_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or_else(|| {
                PostgresError::Validation(format!("unknown project branch: {branch_id}"))
            })?;
        let branch = project_branch_from_row(&branch_row)?;
        if branch.status == "merged" {
            if branch.merge_commit.as_deref() == Some(commit) {
                let query = format!(
                    "{} WHERE m.person_id = $1 AND m.memory_id = $2",
                    memory_select_sql()
                );
                if let Some(merged_memory_id) = &branch.merge_memory_id
                    && let Some(row) = sqlx::query(&query)
                        .bind(&account.person_id)
                        .bind(merged_memory_id)
                        .fetch_optional(&mut *transaction)
                        .await?
                {
                    let memory = memory_from_row(&row)?;
                    if memory.promoted_from.as_deref() == Some(memory_id) {
                        transaction.commit().await?;
                        return Ok(ProjectMergeResult { branch, memory });
                    }
                }
            }
            return Err(PostgresError::Validation(
                "project branch is already merged".to_owned(),
            ));
        }
        if branch.status == "closed" {
            return Err(PostgresError::Validation(
                "closed project branches cannot be merged".to_owned(),
            ));
        }

        let source_query = format!(
            "{} WHERE m.person_id = $1 AND m.memory_id = $2",
            memory_select_sql()
        );
        let source_row = sqlx::query(&source_query)
            .bind(&account.person_id)
            .bind(memory_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or_else(|| PostgresError::Validation(format!("unknown memory: {memory_id}")))?;
        let source = memory_from_row(&source_row)?;
        if source.context.repository.as_deref() != Some(project.repository.as_str()) {
            return Err(PostgresError::Validation(
                "memory repository does not match the project".to_owned(),
            ));
        }
        if source.context.branch.as_deref() != Some(branch.name.as_str()) {
            return Err(PostgresError::Validation(
                "memory branch does not match the registered project branch".to_owned(),
            ));
        }
        let forgotten = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM cairn_forgotten_items
             WHERE person_id = $1 AND item_id = $2)",
        )
        .bind(&account.person_id)
        .bind(memory_id)
        .fetch_one(&mut *transaction)
        .await?;
        if forgotten
            || source
                .lifecycle
                .expires_at
                .as_deref()
                .and_then(|value| value.parse::<u64>().ok())
                .is_some_and(|expires_at| expires_at <= now_seconds())
        {
            return Err(PostgresError::Validation(
                "forgotten or expired memories cannot be promoted".to_owned(),
            ));
        }
        if source.lifecycle.sensitivity == cairn_core::Sensitivity::Secret {
            return Err(PostgresError::Validation(
                "secret memories cannot be promoted".to_owned(),
            ));
        }

        let duplicate_query = format!(
            "{} WHERE m.person_id = $1 AND m.project_id = $2 AND m.promoted_from = $3 AND {}",
            memory_select_sql(),
            live_memory_sql()
        );
        let existing = sqlx::query(&duplicate_query)
            .bind(&account.person_id)
            .bind(project_id)
            .bind(memory_id)
            .fetch_optional(&mut *transaction)
            .await?;
        let mut created_project_memory = false;
        let mut promoted = if let Some(row) = existing {
            memory_from_row(&row)?
        } else {
            let mut promoted = source;
            promoted.memory_id = format!(
                "memory-project-{}",
                &cairn_core::hash_token(&random_token("promotion"))[..24]
            );
            promoted.kind = MemoryKind::Decision;
            promoted.scope = MemoryScope::Project;
            promoted.project_id = Some(project_id.to_owned());
            promoted.context.repository = Some(project.repository.clone());
            promoted.context.branch = None;
            promoted.context.worktree = None;
            promoted.context.commit = None;
            promoted.promoted_from = Some(memory_id.to_owned());
            promoted.source_event_ids.clear();
            promoted.supersedes = prior_project_copy(
                &mut transaction,
                &account.person_id,
                project_id,
                promoted.supersedes.as_deref(),
            )
            .await?;
            promoted.promotion_reason = Some(redact_sensitive_content(reason));
            redact_memory_metadata(&mut promoted);
            promoted.lifecycle.retention = cairn_core::Retention::Durable;
            promoted.lifecycle.expires_at = None;
            promoted.created_at = now_seconds().to_string();
            insert_memory(&mut transaction, &account.person_id, &promoted).await?;
            created_project_memory = true;
            promoted
        };
        let previous_commit = promoted.verified_commit.clone();
        promoted.verified_commit = Some(commit.to_owned());
        promoted.lifecycle.freshness = 100;
        let lifecycle = serde_json::to_value(&promoted.lifecycle)?;
        sqlx::query(
            "UPDATE cairn_memories
             SET verified_commit = $1, lifecycle = $2
             WHERE person_id = $3 AND memory_id = $4",
        )
        .bind(commit)
        .bind(lifecycle)
        .bind(&account.person_id)
        .bind(&promoted.memory_id)
        .execute(&mut *transaction)
        .await?;
        index_project_memory_tx(&mut transaction, &account.person_id, project_id, &promoted)
            .await?;
        insert_freshness_event_tx(
            &mut transaction,
            &account.person_id,
            FreshnessEventInsert {
                project_id,
                memory_id: &promoted.memory_id,
                previous_commit,
                commit,
                action: "merge",
                reason: Some(redact_sensitive_content(reason)),
            },
        )
        .await?;

        let resolved_branch = sqlx::query(
            "UPDATE cairn_project_branches
             SET status = 'merged', status_reason = $1, merge_commit = $2,
                 merge_memory_id = $3, updated_at = now()
             WHERE person_id = $4 AND project_id = $5 AND branch_id = $6
             RETURNING branch_id, project_id, name, base_branch, base_commit, head_commit,
                       status, status_reason, merge_commit, merge_memory_id,
                       EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at,
                       EXTRACT(EPOCH FROM updated_at)::bigint::text AS updated_at",
        )
        .bind(redact_sensitive_content(reason))
        .bind(commit)
        .bind(&promoted.memory_id)
        .bind(&account.person_id)
        .bind(project_id)
        .bind(branch_id)
        .fetch_one(&mut *transaction)
        .await?;
        let resolved_branch = project_branch_from_row(&resolved_branch)?;
        sqlx::query(
            "UPDATE cairn_project_worktrees
             SET status = 'removed', updated_at = now()
             WHERE person_id = $1 AND project_id = $2 AND branch_id = $3",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .bind(branch_id)
        .execute(&mut *transaction)
        .await?;
        record_audit(
            &mut transaction,
            &account.person_id,
            None,
            "branch_merge_confirmed",
            Some(branch_id),
            serde_json::json!({
                "project_id": project_id,
                "memory_id": memory_id,
                "promotion_id": promoted.memory_id,
                "commit": commit
            }),
        )
        .await?;
        if created_project_memory {
            auto_share_project_memory_tx(
                &mut transaction,
                &account.person_id,
                project_id,
                &promoted,
            )
            .await?;
        }
        transaction.commit().await?;
        Ok(ProjectMergeResult {
            branch: resolved_branch,
            memory: promoted,
        })
    }

    pub async fn close_branch(
        &self,
        account_token: &str,
        project_id: &str,
        branch_id: &str,
        reason: &str,
    ) -> Result<ProjectBranch, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        validate_project_text(project_id, "project_id")?;
        validate_project_text(branch_id, "branch_id")?;
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(PostgresError::Validation(
                "branch close reason is required".to_owned(),
            ));
        }
        if reason.len() > 2048 {
            return Err(PostgresError::Validation(
                "branch close reason exceeds the 2048 byte limit".to_owned(),
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let branch_query = format!(
            "{} FOR UPDATE",
            branch_select_sql("b.project_id = $2 AND b.branch_id = $3")
        );
        let branch_row = sqlx::query(&branch_query)
            .bind(&account.person_id)
            .bind(project_id)
            .bind(branch_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or_else(|| {
                PostgresError::Validation(format!("unknown project branch: {branch_id}"))
            })?;
        let branch = project_branch_from_row(&branch_row)?;
        if branch.status == "merged" {
            return Err(PostgresError::Validation(
                "merged project branches cannot be closed".to_owned(),
            ));
        }
        if branch.status == "closed" {
            transaction.commit().await?;
            return Ok(branch);
        }
        let closed_row = sqlx::query(
            "UPDATE cairn_project_branches
             SET status = 'closed', status_reason = $1, updated_at = now()
             WHERE person_id = $2 AND project_id = $3 AND branch_id = $4
             RETURNING branch_id, project_id, name, base_branch, base_commit, head_commit,
                       status, status_reason, merge_commit, merge_memory_id,
                       EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at,
                       EXTRACT(EPOCH FROM updated_at)::bigint::text AS updated_at",
        )
        .bind(redact_sensitive_content(reason))
        .bind(&account.person_id)
        .bind(project_id)
        .bind(branch_id)
        .fetch_one(&mut *transaction)
        .await?;
        let closed = project_branch_from_row(&closed_row)?;
        sqlx::query(
            "UPDATE cairn_project_worktrees
             SET status = 'removed', updated_at = now()
             WHERE person_id = $1 AND project_id = $2 AND branch_id = $3",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .bind(branch_id)
        .execute(&mut *transaction)
        .await?;
        record_audit(
            &mut transaction,
            &account.person_id,
            None,
            "branch_closed",
            Some(branch_id),
            serde_json::json!({ "project_id": project_id }),
        )
        .await?;
        transaction.commit().await?;
        Ok(closed)
    }

    pub async fn project_memories(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<Memory>, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        validate_project_text(project_id, "project_id")?;
        self.require_project_owner(&account.person_id, project_id)
            .await?;
        let query = format!(
            "{} WHERE m.person_id = $1 AND m.project_id = $2
              AND NOT EXISTS (SELECT 1 FROM cairn_forgotten_items f
                              WHERE f.person_id = m.person_id
                                AND (f.item_id = m.memory_id OR f.item_id IN
                                     (SELECT jsonb_array_elements_text(m.source_event_ids))))
              AND {}
              ORDER BY m.created_at DESC, m.memory_id",
            memory_select_sql(),
            not_superseded_by_live_memory_sql(true)
        );
        let rows = sqlx::query(&query)
            .bind(&account.person_id)
            .bind(project_id)
            .fetch_all(&self.pool)
            .await?;
        // The origin link is exposed only while the personal memory is still recalled.
        let live = self.all_memories_for_person(&account.person_id).await?;
        let mut memories = rows
            .iter()
            .map(memory_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        for memory in &mut memories {
            if !live
                .iter()
                .any(|origin| Some(origin.memory_id.as_str()) == memory.promoted_from.as_deref())
            {
                memory.promoted_from = None;
            }
        }
        Ok(memories)
    }

    pub async fn invite_project_member(
        &self,
        account_token: &str,
        project_id: &str,
        email: &str,
    ) -> Result<ProjectMembership, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        self.require_project_owner(&account.person_id, project_id)
            .await?;
        let email = normalize_email(email)?;
        if email == account.email {
            return Err(PostgresError::Validation(
                "project owner cannot invite themselves".to_owned(),
            ));
        }
        let member = self.account_by_email(&email).await?;
        if let Some(row) = sqlx::query(
            "SELECT m.project_id, m.member_person_id, a.email,
                    EXTRACT(EPOCH FROM m.created_at)::bigint::text AS created_at
             FROM cairn_project_members m
             JOIN cairn_accounts a ON a.person_id = m.member_person_id
             WHERE m.owner_person_id = $1 AND m.project_id = $2 AND m.member_person_id = $3",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .bind(&member.person_id)
        .fetch_optional(&self.pool)
        .await?
        {
            return membership_from_row(&row);
        }
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            "INSERT INTO cairn_project_members (owner_person_id, project_id, member_person_id)
             VALUES ($1, $2, $3)
             RETURNING project_id, member_person_id,
                       EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .bind(&member.person_id)
        .fetch_one(&mut *transaction)
        .await?;
        record_audit(
            &mut transaction,
            &account.person_id,
            None,
            "project_member_invited",
            Some(&member.person_id),
            serde_json::json!({
                "project_id": project_id,
                "member_person_id": member.person_id,
            }),
        )
        .await?;
        transaction.commit().await?;
        Ok(ProjectMembership {
            project_id: row.try_get("project_id")?,
            person_id: row.try_get("member_person_id")?,
            email: member.email,
            created_at: row.try_get("created_at")?,
        })
    }

    pub async fn share_project_memory(
        &self,
        account_token: &str,
        project_id: &str,
        memory_id: &str,
        email: &str,
    ) -> Result<ProjectMemoryShare, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        self.require_project_owner(&account.person_id, project_id)
            .await?;
        let member = self.account_by_email(&normalize_email(email)?).await?;
        if member.person_id == account.person_id {
            return Err(PostgresError::Validation(
                "project owner cannot invite themselves".to_owned(),
            ));
        }
        let member_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM cairn_project_members
                WHERE owner_person_id = $1 AND project_id = $2 AND member_person_id = $3
             )",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .bind(&member.person_id)
        .fetch_one(&self.pool)
        .await?;
        if !member_exists {
            return Err(PostgresError::Validation(
                "account is not a project member".to_owned(),
            ));
        }
        self.require_shareable_memory(&account.person_id, project_id, memory_id)
            .await?;
        if let Some(row) = self
            .share_row(
                &account.person_id,
                project_id,
                memory_id,
                &member.person_id,
                true,
            )
            .await?
        {
            return share_from_row(&row);
        }
        let mut transaction = self.pool.begin().await?;
        let reactivated = sqlx::query(
            "UPDATE cairn_project_shares
             SET revoked_at = NULL, share_reason = $5
             WHERE owner_person_id = $1 AND project_id = $2 AND memory_id = $3
               AND member_person_id = $4 AND revoked_at IS NOT NULL
             RETURNING project_id, memory_id, member_person_id,
                       EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at,
                       EXTRACT(EPOCH FROM revoked_at)::bigint::text AS revoked_at",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .bind(memory_id)
        .bind(&member.person_id)
        .bind(SHARE_REASON_MANUAL)
        .fetch_optional(&mut *transaction)
        .await?;
        let row = if let Some(row) = reactivated {
            row
        } else {
            sqlx::query(
                "INSERT INTO cairn_project_shares
                    (owner_person_id, project_id, memory_id, member_person_id, share_reason)
                 VALUES ($1, $2, $3, $4, $5)
                 RETURNING project_id, memory_id, member_person_id,
                           EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at,
                           EXTRACT(EPOCH FROM revoked_at)::bigint::text AS revoked_at",
            )
            .bind(&account.person_id)
            .bind(project_id)
            .bind(memory_id)
            .bind(&member.person_id)
            .bind(SHARE_REASON_MANUAL)
            .fetch_one(&mut *transaction)
            .await?
        };
        record_audit(
            &mut transaction,
            &account.person_id,
            None,
            "project_memory_shared",
            Some(memory_id),
            serde_json::json!({
                "project_id": project_id,
                "member_person_id": member.person_id,
                "memory_id": memory_id,
            }),
        )
        .await?;
        transaction.commit().await?;
        share_from_row(&row)
    }

    pub async fn shared_project_memories(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<SharedProjectMemory>, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        validate_project_text(project_id, "project_id")?;
        let project_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM cairn_projects WHERE project_id = $1)",
        )
        .bind(project_id)
        .fetch_one(&self.pool)
        .await?;
        if !project_exists {
            return Err(PostgresError::Validation(format!(
                "unknown project: {project_id}"
            )));
        }
        let is_member = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM cairn_project_members
                WHERE project_id = $1 AND member_person_id = $2
             )",
        )
        .bind(project_id)
        .bind(&account.person_id)
        .fetch_one(&self.pool)
        .await?;
        if !is_member {
            let had_share = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (
                    SELECT 1 FROM cairn_project_shares
                    WHERE project_id = $1 AND member_person_id = $2
                 )",
            )
            .bind(project_id)
            .bind(&account.person_id)
            .fetch_one(&self.pool)
            .await?;
            if had_share {
                return Ok(Vec::new());
            }
            return Err(PostgresError::Validation(
                "account is not a project member".to_owned(),
            ));
        }
        let query = format!(
            "{} JOIN cairn_project_shares s
                ON s.memory_id = m.memory_id
               AND s.owner_person_id = m.person_id
               AND s.project_id = m.project_id
              WHERE s.member_person_id = $1 AND s.project_id = $2 AND s.revoked_at IS NULL
                AND m.scope = 'project'
                AND NOT EXISTS (
                    SELECT 1 FROM cairn_forgotten_items f
                    WHERE f.person_id = m.person_id
                      AND (f.item_id = m.memory_id OR f.item_id IN
                           (SELECT jsonb_array_elements_text(m.source_event_ids)))
                )
                AND (m.lifecycle->>'expiresAt' IS NULL OR
                     (m.lifecycle->>'expiresAt')::bigint > EXTRACT(EPOCH FROM now())::bigint)
                AND {}
              ORDER BY m.created_at DESC, m.memory_id",
            shared_memory_select_sql(),
            not_superseded_by_live_memory_sql(true)
        );
        let rows = sqlx::query(&query)
            .bind(&account.person_id)
            .bind(project_id)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(shared_memory_from_row).collect()
    }

    pub async fn revoke_project_share(
        &self,
        account_token: &str,
        project_id: &str,
        memory_id: &str,
        email: &str,
    ) -> Result<ProjectMemoryShare, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        self.require_project_owner(&account.person_id, project_id)
            .await?;
        let member = self.account_by_email(&normalize_email(email)?).await?;
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            "UPDATE cairn_project_shares
             SET revoked_at = now()
             WHERE owner_person_id = $1 AND project_id = $2 AND memory_id = $3
               AND member_person_id = $4 AND revoked_at IS NULL
             RETURNING project_id, memory_id, member_person_id,
                       EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at,
                       EXTRACT(EPOCH FROM revoked_at)::bigint::text AS revoked_at",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .bind(memory_id)
        .bind(&member.person_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| PostgresError::Validation("unknown project share".to_owned()))?;
        record_audit(
            &mut transaction,
            &account.person_id,
            None,
            "project_memory_share_revoked",
            Some(memory_id),
            serde_json::json!({
                "project_id": project_id,
                "member_person_id": member.person_id,
                "memory_id": memory_id,
            }),
        )
        .await?;
        transaction.commit().await?;
        share_from_row(&row)
    }

    pub async fn create_project_team(
        &self,
        account_token: &str,
        project_id: &str,
        name: &str,
    ) -> Result<ProjectTeam, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        self.require_project_owner(&account.person_id, project_id)
            .await?;
        let name = name.trim();
        validate_project_text(name, "name")?;
        if let Some(existing) = self.project_team_row(project_id).await? {
            let existing_name: String = existing.try_get("name")?;
            if existing_name == name {
                return self.project_team_view(project_id).await;
            }
            return Err(PostgresError::Validation(
                "project already has a team space".to_owned(),
            ));
        }
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO cairn_project_teams (owner_person_id, project_id, name)
             VALUES ($1, $2, $3)",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .bind(name)
        .execute(&mut *transaction)
        .await?;
        record_audit(
            &mut transaction,
            &account.person_id,
            None,
            "project_team_created",
            Some(project_id),
            serde_json::json!({ "project_id": project_id }),
        )
        .await?;
        transaction.commit().await?;
        self.project_team_view(project_id).await
    }

    pub async fn add_project_team_member(
        &self,
        account_token: &str,
        project_id: &str,
        email: &str,
    ) -> Result<ProjectTeamMember, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        self.require_project_owner(&account.person_id, project_id)
            .await?;
        let team_exists = self.project_team_row(project_id).await?.is_some();
        if !team_exists {
            return Err(PostgresError::Validation(
                "project has no team space".to_owned(),
            ));
        }
        let member = self.account_by_email(&normalize_email(email)?).await?;
        let member_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM cairn_project_members
                WHERE owner_person_id = $1 AND project_id = $2 AND member_person_id = $3
             )",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .bind(&member.person_id)
        .fetch_one(&self.pool)
        .await?;
        if !member_exists {
            return Err(PostgresError::Validation(
                "account is not a project member".to_owned(),
            ));
        }
        if let Some(row) = self.team_member_row(project_id, &member.person_id).await? {
            return team_member_from_row(&row);
        }
        let mut transaction = self.pool.begin().await?;
        let row = sqlx::query(
            "INSERT INTO cairn_project_team_members
                (owner_person_id, project_id, member_person_id)
             VALUES ($1, $2, $3)
             RETURNING member_person_id,
                       EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .bind(&member.person_id)
        .fetch_one(&mut *transaction)
        .await?;
        record_audit(
            &mut transaction,
            &account.person_id,
            None,
            "project_team_member_added",
            Some(&member.person_id),
            serde_json::json!({
                "project_id": project_id,
                "member_person_id": member.person_id,
            }),
        )
        .await?;
        transaction.commit().await?;
        Ok(ProjectTeamMember {
            person_id: row.try_get("member_person_id")?,
            email: member.email,
            created_at: row.try_get("created_at")?,
        })
    }

    pub async fn remove_project_team_member(
        &self,
        account_token: &str,
        project_id: &str,
        email: &str,
    ) -> Result<ProjectTeamMember, PostgresError> {
        // Removing a member reopens the solo-project merge path, so it stays web-only.
        let account = self.authenticate_session(account_token).await?;
        self.require_project_owner(&account.person_id, project_id)
            .await?;
        let member = self.account_by_email(&normalize_email(email)?).await?;
        let row = self
            .team_member_row(project_id, &member.person_id)
            .await?
            .ok_or_else(|| PostgresError::Validation("account is not a team member".to_owned()))?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "UPDATE cairn_project_shares
             SET revoked_at = now()
             WHERE project_id = $1 AND member_person_id = $2 AND revoked_at IS NULL",
        )
        .bind(project_id)
        .bind(&member.person_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "DELETE FROM cairn_project_members
             WHERE project_id = $1 AND member_person_id = $2",
        )
        .bind(project_id)
        .bind(&member.person_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "DELETE FROM cairn_project_team_members
             WHERE project_id = $1 AND member_person_id = $2",
        )
        .bind(project_id)
        .bind(&member.person_id)
        .execute(&mut *transaction)
        .await?;
        record_audit(
            &mut transaction,
            &account.person_id,
            None,
            "project_team_member_removed",
            Some(&member.person_id),
            serde_json::json!({
                "project_id": project_id,
                "member_person_id": member.person_id,
            }),
        )
        .await?;
        transaction.commit().await?;
        team_member_from_row(&row)
    }

    pub async fn project_team(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<ProjectTeam, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        self.require_project_reader(&account.person_id, project_id)
            .await?;
        if self.project_team_row(project_id).await?.is_none() {
            return Err(PostgresError::Validation(
                "project has no team space".to_owned(),
            ));
        }
        self.project_team_view(project_id).await
    }

    pub async fn set_project_share_policy(
        &self,
        account_token: &str,
        project_id: &str,
        policy: &str,
    ) -> Result<ProjectSharePolicyRecord, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        self.require_project_owner(&account.person_id, project_id)
            .await?;
        let policy = ProjectSharePolicy::parse(policy)?;
        sqlx::query(
            "INSERT INTO cairn_project_share_policies (owner_person_id, project_id, policy)
             VALUES ($1, $2, $3)
             ON CONFLICT (owner_person_id, project_id)
             DO UPDATE SET policy = EXCLUDED.policy, updated_at = now()",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .bind(policy_name(&policy))
        .execute(&self.pool)
        .await?;
        Ok(ProjectSharePolicyRecord {
            project_id: project_id.to_owned(),
            policy,
        })
    }

    /// The owner's current share policy for the project.
    pub async fn share_policy(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<ProjectSharePolicy, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        self.require_project_owner(&account.person_id, project_id)
            .await?;
        self.load_share_policy(&account.person_id, project_id).await
    }

    pub async fn share_candidates(
        &self,
        account_token: &str,
        project_id: &str,
        email: &str,
    ) -> Result<Vec<ProjectShareCandidate>, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        self.require_project_owner(&account.person_id, project_id)
            .await?;
        let member = self.account_by_email(&normalize_email(email)?).await?;
        let member_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM cairn_project_members
                WHERE owner_person_id = $1 AND project_id = $2 AND member_person_id = $3
             )",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .bind(&member.person_id)
        .fetch_one(&self.pool)
        .await?;
        if !member_exists {
            return Err(PostgresError::Validation(
                "account is not a project member".to_owned(),
            ));
        }
        let policy = self
            .load_share_policy(&account.person_id, project_id)
            .await?;
        let shared_ids = sqlx::query_scalar::<_, String>(
            "SELECT memory_id FROM cairn_project_shares
             WHERE owner_person_id = $1 AND project_id = $2 AND member_person_id = $3
               AND revoked_at IS NULL",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .bind(&member.person_id)
        .fetch_all(&self.pool)
        .await?;
        let dismissed_ids = sqlx::query_scalar::<_, String>(
            "SELECT memory_id FROM cairn_share_candidate_dismissals
             WHERE owner_person_id = $1 AND project_id = $2 AND member_person_id = $3",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .bind(&member.person_id)
        .fetch_all(&self.pool)
        .await?;
        let memories = self.project_memories(account_token, project_id).await?;
        let mut candidates = Vec::new();
        for memory in memories {
            if memory.scope != MemoryScope::Project
                || memory.project_id.as_deref() != Some(project_id)
                || memory.lifecycle.sensitivity == cairn_core::Sensitivity::Secret
                || shared_ids.iter().any(|id| id == &memory.memory_id)
                || dismissed_ids.iter().any(|id| id == &memory.memory_id)
            {
                continue;
            }
            candidates.push(ProjectShareCandidate {
                memory_id: memory.memory_id.clone(),
                reason: "promoted project memory is not shared with the member".to_owned(),
                not_auto_shared_reason: postgres_not_auto_shared_reason(&policy, &memory),
            });
        }
        Ok(candidates)
    }

    pub async fn dismiss_share_candidate(
        &self,
        account_token: &str,
        project_id: &str,
        memory_id: &str,
        email: &str,
    ) -> Result<ShareCandidateDismissal, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        self.require_project_owner(&account.person_id, project_id)
            .await?;
        let member = self.account_by_email(&normalize_email(email)?).await?;
        let member_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM cairn_project_members
                WHERE owner_person_id = $1 AND project_id = $2 AND member_person_id = $3
             )",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .bind(&member.person_id)
        .fetch_one(&self.pool)
        .await?;
        if !member_exists {
            return Err(PostgresError::Validation(
                "account is not a project member".to_owned(),
            ));
        }
        let project_memory = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM cairn_memories
                WHERE person_id = $1 AND project_id = $2 AND memory_id = $3 AND scope = 'project'
             )",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .bind(memory_id)
        .fetch_one(&self.pool)
        .await?;
        if !project_memory {
            return Err(PostgresError::Validation(
                "personal observations and non-project memories cannot be shared".to_owned(),
            ));
        }
        sqlx::query(
            "INSERT INTO cairn_share_candidate_dismissals
                (owner_person_id, project_id, memory_id, member_person_id)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (owner_person_id, project_id, memory_id, member_person_id) DO NOTHING",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .bind(memory_id)
        .bind(&member.person_id)
        .execute(&self.pool)
        .await?;
        Ok(ShareCandidateDismissal {
            project_id: project_id.to_owned(),
            memory_id: memory_id.to_owned(),
            dismissed: true,
        })
    }

    pub async fn project_conflicts(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectConflict>, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        validate_project_text(project_id, "project_id")?;
        let owns = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM cairn_projects WHERE person_id = $1 AND project_id = $2
             )",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .fetch_one(&self.pool)
        .await?;
        let memories = if owns {
            self.project_memories(account_token, project_id).await?
        } else {
            self.shared_project_memories(account_token, project_id)
                .await?
                .into_iter()
                .map(|shared| shared.memory)
                .collect()
        };
        Ok(competing_project_memories(&memories))
    }

    pub async fn project_revocations(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectMemoryShare>, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        validate_project_text(project_id, "project_id")?;
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM cairn_projects WHERE project_id = $1)",
        )
        .bind(project_id)
        .fetch_one(&self.pool)
        .await?;
        if !exists {
            return Err(PostgresError::Validation(format!(
                "unknown project: {project_id}"
            )));
        }
        let owns = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM cairn_projects WHERE person_id = $1 AND project_id = $2
             )",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .fetch_one(&self.pool)
        .await?;
        let member = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM cairn_project_members
                WHERE project_id = $1 AND member_person_id = $2
             )",
        )
        .bind(project_id)
        .bind(&account.person_id)
        .fetch_one(&self.pool)
        .await?;
        if !owns && !member {
            return Err(PostgresError::Validation(
                "account does not own the project".to_owned(),
            ));
        }
        let rows = if owns {
            sqlx::query(
                "SELECT s.project_id, s.memory_id, s.member_person_id, a.email AS member_email,
                        EXTRACT(EPOCH FROM s.created_at)::bigint::text AS created_at,
                        EXTRACT(EPOCH FROM s.revoked_at)::bigint::text AS revoked_at
                 FROM cairn_project_shares s
                 LEFT JOIN cairn_accounts a ON a.person_id = s.member_person_id
                 WHERE s.owner_person_id = $1 AND s.project_id = $2 AND s.revoked_at IS NOT NULL
                 ORDER BY s.revoked_at DESC",
            )
            .bind(&account.person_id)
            .bind(project_id)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT s.project_id, s.memory_id, s.member_person_id, a.email AS member_email,
                        EXTRACT(EPOCH FROM s.created_at)::bigint::text AS created_at,
                        EXTRACT(EPOCH FROM s.revoked_at)::bigint::text AS revoked_at
                 FROM cairn_project_shares s
                 LEFT JOIN cairn_accounts a ON a.person_id = s.member_person_id
                 WHERE s.member_person_id = $1 AND s.project_id = $2 AND s.revoked_at IS NOT NULL
                 ORDER BY s.revoked_at DESC",
            )
            .bind(&account.person_id)
            .bind(project_id)
            .fetch_all(&self.pool)
            .await?
        };
        rows.iter()
            .map(|row| {
                Ok(ProjectMemoryShare {
                    member_email: row.try_get("member_email")?,
                    ..share_from_row(row)?
                })
            })
            .collect()
    }

    async fn load_share_policy(
        &self,
        owner_person_id: &str,
        project_id: &str,
    ) -> Result<ProjectSharePolicy, PostgresError> {
        let policy = sqlx::query_scalar::<_, String>(
            "SELECT policy FROM cairn_project_share_policies
             WHERE owner_person_id = $1 AND project_id = $2",
        )
        .bind(owner_person_id)
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(match policy {
            Some(value) => ProjectSharePolicy::parse(&value)?,
            None => ProjectSharePolicy::Manual,
        })
    }

    async fn project_team_row(&self, project_id: &str) -> Result<Option<PgRow>, PostgresError> {
        Ok(sqlx::query(
            "SELECT project_id, name,
                    EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at
             FROM cairn_project_teams WHERE project_id = $1",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    async fn team_member_row(
        &self,
        project_id: &str,
        member_person_id: &str,
    ) -> Result<Option<PgRow>, PostgresError> {
        Ok(sqlx::query(
            "SELECT m.member_person_id, a.email,
                    EXTRACT(EPOCH FROM m.created_at)::bigint::text AS created_at
             FROM cairn_project_team_members m
             JOIN cairn_accounts a ON a.person_id = m.member_person_id
             WHERE m.project_id = $1 AND m.member_person_id = $2",
        )
        .bind(project_id)
        .bind(member_person_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    async fn project_team_view(&self, project_id: &str) -> Result<ProjectTeam, PostgresError> {
        let team = self
            .project_team_row(project_id)
            .await?
            .ok_or_else(|| PostgresError::Validation("project has no team space".to_owned()))?;
        let rows = sqlx::query(
            "SELECT m.member_person_id, a.email,
                    EXTRACT(EPOCH FROM m.created_at)::bigint::text AS created_at
             FROM cairn_project_team_members m
             JOIN cairn_accounts a ON a.person_id = m.member_person_id
             WHERE m.project_id = $1
             ORDER BY m.created_at, m.member_person_id",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        let members = rows
            .iter()
            .map(team_member_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ProjectTeam {
            project_id: team.try_get("project_id")?,
            name: team.try_get("name")?,
            created_at: team.try_get("created_at")?,
            members,
        })
    }

    async fn require_project_reader(
        &self,
        person_id: &str,
        project_id: &str,
    ) -> Result<(), PostgresError> {
        validate_project_text(project_id, "project_id")?;
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM cairn_projects WHERE project_id = $1)",
        )
        .bind(project_id)
        .fetch_one(&self.pool)
        .await?;
        if !exists {
            return Err(PostgresError::Validation(format!(
                "unknown project: {project_id}"
            )));
        }
        let is_owner = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM cairn_projects WHERE person_id = $1 AND project_id = $2
             )",
        )
        .bind(person_id)
        .bind(project_id)
        .fetch_one(&self.pool)
        .await?;
        if is_owner {
            return Ok(());
        }
        let is_member = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM cairn_project_members
                WHERE project_id = $1 AND member_person_id = $2
             )",
        )
        .bind(project_id)
        .bind(person_id)
        .fetch_one(&self.pool)
        .await?;
        if is_member {
            return Ok(());
        }
        Err(PostgresError::Validation(
            "account is not a project member".to_owned(),
        ))
    }

    /// The caller's own project, with one error for a missing and a foreign id so a request
    /// never reaches a foreign-key failure. The database key still guards a concurrent delete.
    async fn require_own_project(
        &self,
        person_id: &str,
        project_id: &str,
    ) -> Result<(), PostgresError> {
        let owns = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM cairn_projects
             WHERE person_id = $1 AND project_id = $2)",
        )
        .bind(person_id)
        .bind(project_id)
        .fetch_one(&self.pool)
        .await?;
        if owns {
            return Ok(());
        }
        Err(PostgresError::Validation(format!(
            "unknown project: {project_id}"
        )))
    }

    async fn require_project_owner(
        &self,
        person_id: &str,
        project_id: &str,
    ) -> Result<(), PostgresError> {
        let owns = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM cairn_projects WHERE person_id = $1 AND project_id = $2
             )",
        )
        .bind(person_id)
        .bind(project_id)
        .fetch_one(&self.pool)
        .await?;
        if owns {
            return Ok(());
        }
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM cairn_projects WHERE project_id = $1)",
        )
        .bind(project_id)
        .fetch_one(&self.pool)
        .await?;
        if exists {
            return Err(PostgresError::Validation(
                "account does not own the project".to_owned(),
            ));
        }
        Err(PostgresError::Validation(format!(
            "unknown project: {project_id}"
        )))
    }

    async fn account_by_email(&self, email: &str) -> Result<Account, PostgresError> {
        let row = sqlx::query(
            "SELECT person_id, email, status, is_admin,
                    EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at
             FROM cairn_accounts WHERE email = $1",
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| PostgresError::Validation("unknown account email".to_owned()))?;
        account_from_row(&row)
    }

    async fn require_shareable_memory(
        &self,
        person_id: &str,
        project_id: &str,
        memory_id: &str,
    ) -> Result<(), PostgresError> {
        let query = format!(
            "{} WHERE m.person_id = $1 AND m.memory_id = $2",
            memory_select_sql()
        );
        let row = sqlx::query(&query)
            .bind(person_id)
            .bind(memory_id)
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            let observation = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (
                    SELECT 1 FROM cairn_observations
                    WHERE person_id = $1 AND event_id = $2
                 )",
            )
            .bind(person_id)
            .bind(memory_id)
            .fetch_one(&self.pool)
            .await?;
            if observation {
                return Err(PostgresError::Validation(
                    "personal observations and non-project memories cannot be shared".to_owned(),
                ));
            }
            return Err(PostgresError::Validation(format!(
                "unknown memory: {memory_id}"
            )));
        };
        let memory = memory_from_row(&row)?;
        let forgotten = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM cairn_forgotten_items
                WHERE person_id = $1 AND item_id = $2
             )",
        )
        .bind(person_id)
        .bind(memory_id)
        .fetch_one(&self.pool)
        .await?;
        if forgotten
            || memory
                .lifecycle
                .expires_at
                .as_deref()
                .and_then(|value| value.parse::<u64>().ok())
                .is_some_and(|expires_at| expires_at <= now_seconds())
        {
            return Err(PostgresError::Validation(
                "forgotten or expired memories cannot be shared".to_owned(),
            ));
        }
        if memory.lifecycle.sensitivity == cairn_core::Sensitivity::Secret {
            return Err(PostgresError::Validation(
                "secret memories cannot be shared".to_owned(),
            ));
        }
        if memory.scope == MemoryScope::Project && memory.project_id.as_deref() == Some(project_id)
        {
            return Ok(());
        }
        if memory.scope == MemoryScope::Project && memory.project_id.is_some() {
            return Err(PostgresError::Validation(
                "memory does not belong to the project".to_owned(),
            ));
        }
        Err(PostgresError::Validation(
            "personal observations and non-project memories cannot be shared".to_owned(),
        ))
    }

    async fn share_row(
        &self,
        owner_person_id: &str,
        project_id: &str,
        memory_id: &str,
        member_person_id: &str,
        active_only: bool,
    ) -> Result<Option<PgRow>, PostgresError> {
        let revoked_clause = if active_only {
            "AND revoked_at IS NULL"
        } else {
            ""
        };
        let sql = format!(
            "SELECT project_id, memory_id, member_person_id,
                    EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at,
                    EXTRACT(EPOCH FROM revoked_at)::bigint::text AS revoked_at
             FROM cairn_project_shares
             WHERE owner_person_id = $1 AND project_id = $2 AND memory_id = $3
               AND member_person_id = $4 {revoked_clause}"
        );
        Ok(sqlx::query(&sql)
            .bind(owner_person_id)
            .bind(project_id)
            .bind(memory_id)
            .bind(member_person_id)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn promote_memory(
        &self,
        account_token: &str,
        project_id: &str,
        memory_id: &str,
        reason: &str,
    ) -> Result<Memory, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        if project_id.trim().is_empty() || memory_id.trim().is_empty() || reason.trim().is_empty() {
            return Err(PostgresError::Validation(
                "project_id, memory_id, and promotion reason are required".to_owned(),
            ));
        }
        if reason.len() > 2048 {
            return Err(PostgresError::Validation(
                "promotion reason exceeds the 2048 byte limit".to_owned(),
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let project = sqlx::query(
            "SELECT project_id, repository, name, default_branch,
                    EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at,
                    EXTRACT(EPOCH FROM updated_at)::bigint::text AS updated_at
             FROM cairn_projects WHERE person_id = $1 AND project_id = $2",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| PostgresError::Validation(format!("unknown project: {project_id}")))?;
        let project = project_from_row(&project)?;
        let source_query = format!(
            "{} WHERE m.person_id = $1 AND m.memory_id = $2",
            memory_select_sql()
        );
        let source_row = sqlx::query(&source_query)
            .bind(&account.person_id)
            .bind(memory_id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or_else(|| PostgresError::Validation(format!("unknown memory: {memory_id}")))?;
        let source = memory_from_row(&source_row)?;
        let source_commit = source.context.commit.clone();
        let forgotten = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM cairn_forgotten_items
             WHERE person_id = $1 AND item_id = $2)",
        )
        .bind(&account.person_id)
        .bind(memory_id)
        .fetch_one(&mut *transaction)
        .await?;
        if forgotten
            || source
                .lifecycle
                .expires_at
                .as_deref()
                .and_then(|value| value.parse::<u64>().ok())
                .is_some_and(|expires_at| expires_at <= now_seconds())
        {
            return Err(PostgresError::Validation(
                "forgotten or expired memories cannot be promoted".to_owned(),
            ));
        }
        if source.lifecycle.sensitivity == cairn_core::Sensitivity::Secret {
            return Err(PostgresError::Validation(
                "secret memories cannot be promoted".to_owned(),
            ));
        }
        if source.context.repository.as_deref() != Some(project.repository.as_str()) {
            return Err(PostgresError::Validation(
                "memory repository does not match the project".to_owned(),
            ));
        }
        let duplicate_query = format!(
            "{} WHERE m.person_id = $1 AND m.project_id = $2 AND m.promoted_from = $3 AND {}",
            memory_select_sql(),
            live_memory_sql()
        );
        if let Some(row) = sqlx::query(&duplicate_query)
            .bind(&account.person_id)
            .bind(project_id)
            .bind(memory_id)
            .fetch_optional(&mut *transaction)
            .await?
        {
            return memory_from_row(&row);
        }
        let mut promoted = source;
        promoted.memory_id = format!(
            "memory-project-{}",
            &cairn_core::hash_token(&random_token("promotion"))[..24]
        );
        promoted.kind = MemoryKind::Decision;
        promoted.scope = MemoryScope::Project;
        promoted.project_id = Some(project_id.to_owned());
        promoted.context.repository = Some(project.repository);
        promoted.context.branch = None;
        promoted.context.worktree = None;
        promoted.context.commit = None;
        promoted.promoted_from = Some(memory_id.to_owned());
        promoted.source_event_ids.clear();
        promoted.supersedes = prior_project_copy(
            &mut transaction,
            &account.person_id,
            project_id,
            promoted.supersedes.as_deref(),
        )
        .await?;
        promoted.promotion_reason = Some(redact_sensitive_content(reason));
        redact_memory_metadata(&mut promoted);
        promoted.verified_commit = source_commit;
        promoted.lifecycle.retention = cairn_core::Retention::Durable;
        promoted.lifecycle.expires_at = None;
        promoted.created_at = now_seconds().to_string();
        insert_memory(&mut transaction, &account.person_id, &promoted).await?;
        index_project_memory_tx(&mut transaction, &account.person_id, project_id, &promoted)
            .await?;
        if let Some(commit) = &promoted.verified_commit {
            insert_freshness_event_tx(
                &mut transaction,
                &account.person_id,
                FreshnessEventInsert {
                    project_id,
                    memory_id: &promoted.memory_id,
                    previous_commit: None,
                    commit,
                    action: "promotion",
                    reason: Some(redact_sensitive_content(reason)),
                },
            )
            .await?;
        }
        auto_share_project_memory_tx(&mut transaction, &account.person_id, project_id, &promoted)
            .await?;
        record_audit(
            &mut transaction,
            &account.person_id,
            None,
            "memory_promote",
            Some(memory_id),
            serde_json::json!({ "project_id": project_id, "promotion_id": promoted.memory_id }),
        )
        .await?;
        transaction.commit().await?;
        Ok(promoted)
    }

    pub async fn confirm_memory(
        &self,
        account_token: &str,
        project_id: &str,
        memory_id: &str,
        commit: Option<&str>,
        reason: &str,
    ) -> Result<Memory, PostgresError> {
        let promoted = self
            .promote_memory(account_token, project_id, memory_id, reason)
            .await?;
        match commit.map(str::trim).filter(|value| !value.is_empty()) {
            Some(commit) => {
                self.verify_project_memory(account_token, project_id, &promoted.memory_id, commit)
                    .await
            }
            None => Ok(promoted),
        }
    }

    pub async fn verify_project_memory(
        &self,
        account_token: &str,
        project_id: &str,
        memory_id: &str,
        commit: &str,
    ) -> Result<Memory, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        let commit = commit.trim();
        if project_id.trim().is_empty() || memory_id.trim().is_empty() || commit.is_empty() {
            return Err(PostgresError::Validation(
                "project_id, memory_id, and commit are required".to_owned(),
            ));
        }
        if commit.len() > 256 {
            return Err(PostgresError::Validation(
                "commit exceeds the 256 byte limit".to_owned(),
            ));
        }
        let mut transaction = self.pool.begin().await?;
        let previous_commit = sqlx::query_scalar::<_, Option<String>>(
            "SELECT verified_commit FROM cairn_memories
             WHERE person_id = $1 AND project_id = $2 AND memory_id = $3
               AND scope = 'project'",
        )
        .bind(&account.person_id)
        .bind(project_id)
        .bind(memory_id)
        .fetch_optional(&mut *transaction)
        .await?
        .flatten();
        let result = sqlx::query(
            "UPDATE cairn_memories
             SET verified_commit = $1,
                 lifecycle = jsonb_set(lifecycle, '{freshness}', '100'::jsonb, true)
             WHERE person_id = $2 AND project_id = $3 AND memory_id = $4
               AND scope = 'project'",
        )
        .bind(commit)
        .bind(&account.person_id)
        .bind(project_id)
        .bind(memory_id)
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() == 0 {
            return Err(PostgresError::Validation(format!(
                "unknown project memory: {memory_id}"
            )));
        }
        let query = format!(
            "{} WHERE m.person_id = $1 AND m.memory_id = $2",
            memory_select_sql()
        );
        let row = sqlx::query(&query)
            .bind(&account.person_id)
            .bind(memory_id)
            .fetch_one(&mut *transaction)
            .await?;
        let memory = memory_from_row(&row)?;
        insert_freshness_event_tx(
            &mut transaction,
            &account.person_id,
            FreshnessEventInsert {
                project_id,
                memory_id,
                previous_commit,
                commit,
                action: "verification",
                reason: None,
            },
        )
        .await?;
        record_audit(
            &mut transaction,
            &account.person_id,
            None,
            "memory_verify",
            Some(memory_id),
            serde_json::json!({ "project_id": project_id, "commit": commit }),
        )
        .await?;
        transaction.commit().await?;
        Ok(memory)
    }

    pub async fn export_bundle(&self, account_token: &str) -> Result<ExportBundle, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        let session_rows = sqlx::query(
            "SELECT session_id,
                    EXTRACT(EPOCH FROM started_at)::bigint::text AS started_at,
                    EXTRACT(EPOCH FROM ended_at)::bigint::text AS ended_at,
                    context
             FROM cairn_sessions WHERE person_id = $1 ORDER BY started_at, session_id",
        )
        .bind(&account.person_id)
        .fetch_all(&self.pool)
        .await?;
        let observation_query = format!(
            "{} WHERE o.person_id = $1 ORDER BY o.captured_at, o.event_id",
            observation_select_sql()
        );
        let observation_rows = sqlx::query(&observation_query)
            .bind(&account.person_id)
            .fetch_all(&self.pool)
            .await?;
        let memory_query = format!(
            "{} WHERE m.person_id = $1 ORDER BY m.created_at, m.memory_id",
            memory_select_sql()
        );
        let memory_rows = sqlx::query(&memory_query)
            .bind(&account.person_id)
            .fetch_all(&self.pool)
            .await?;
        let forgotten_rows = sqlx::query(
            "SELECT item_id FROM cairn_forgotten_items
             WHERE person_id = $1 ORDER BY item_id",
        )
        .bind(&account.person_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(ExportBundle {
            format: cairn_core::EXPORT_FORMAT.to_owned(),
            version: cairn_core::EXPORT_VERSION,
            exported_at: now_seconds().to_string(),
            sessions: session_rows
                .iter()
                .map(session_from_row)
                .collect::<Result<Vec<_>, _>>()?,
            observations: observation_rows
                .iter()
                .map(observation_from_row)
                .collect::<Result<Vec<_>, _>>()?,
            memories: memory_rows
                .iter()
                .map(memory_from_row)
                .collect::<Result<Vec<_>, _>>()?,
            forgotten_ids: forgotten_rows
                .iter()
                .map(|row| row.try_get("item_id"))
                .collect::<Result<Vec<String>, sqlx::Error>>()?,
        })
    }

    pub async fn import_bundle(
        &self,
        account_token: &str,
        mut bundle: ExportBundle,
    ) -> Result<ImportResult, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        normalize_export_bundle(&mut bundle)?;
        let mut transaction = self.pool.begin().await?;
        let mut result = ImportResult {
            imported_sessions: 0,
            imported_observations: 0,
            imported_memories: 0,
            imported_forgotten_ids: 0,
        };
        for session in &bundle.sessions {
            let existing = sqlx::query(
                "SELECT session_id,
                        EXTRACT(EPOCH FROM started_at)::bigint::text AS started_at,
                        EXTRACT(EPOCH FROM ended_at)::bigint::text AS ended_at,
                        context
                 FROM cairn_sessions WHERE person_id = $1 AND session_id = $2",
            )
            .bind(&account.person_id)
            .bind(&session.session_id)
            .fetch_optional(&mut *transaction)
            .await?;
            if let Some(row) = existing {
                // The authenticated account is the PostgreSQL owner. A file bundle may
                // carry person_id; that field must not false-conflict this row.
                let mut stored = session_from_row(&row)?;
                stored.person_id.clone_from(&session.person_id);
                if stored != *session {
                    return Err(import_conflict("session", &session.session_id));
                }
            } else {
                sqlx::query(
                    "INSERT INTO cairn_sessions (session_id, person_id, context, started_at, ended_at)
                     VALUES ($1, $2, $3, to_timestamp($4::double precision),
                             CASE WHEN $5::text IS NULL THEN NULL ELSE to_timestamp($5::double precision) END)",
                )
                .bind(&session.session_id)
                .bind(&account.person_id)
                .bind(serde_json::to_value(&session.context)?)
                .bind(session.started_at.parse::<i64>().unwrap_or_else(|_| now_seconds() as i64))
                .bind(session.ended_at.as_deref().and_then(|value| value.parse::<i64>().ok()))
                .execute(&mut *transaction)
                .await?;
                result.imported_sessions += 1;
            }
        }
        for observation in &bundle.observations {
            let query = format!(
                "{} WHERE o.person_id = $1 AND o.event_id = $2",
                observation_select_sql()
            );
            let existing = sqlx::query(&query)
                .bind(&account.person_id)
                .bind(&observation.event_id)
                .fetch_optional(&mut *transaction)
                .await?;
            if let Some(row) = existing {
                if observation_from_row(&row)? != *observation {
                    return Err(import_conflict("observation", &observation.event_id));
                }
            } else {
                insert_observation(&mut transaction, &account.person_id, observation).await?;
                result.imported_observations += 1;
            }
        }
        for memory in &bundle.memories {
            let query = format!(
                "{} WHERE m.person_id = $1 AND m.memory_id = $2",
                memory_select_sql()
            );
            let existing = sqlx::query(&query)
                .bind(&account.person_id)
                .bind(&memory.memory_id)
                .fetch_optional(&mut *transaction)
                .await?;
            if let Some(row) = existing {
                if memory_from_row(&row)? != *memory {
                    return Err(import_conflict("memory", &memory.memory_id));
                }
            } else {
                insert_memory(&mut transaction, &account.person_id, memory).await?;
                result.imported_memories += 1;
            }
        }
        for forgotten_id in &bundle.forgotten_ids {
            let inserted = sqlx::query(
                "INSERT INTO cairn_forgotten_items (person_id, item_id)
                 VALUES ($1, $2) ON CONFLICT (person_id, item_id) DO NOTHING",
            )
            .bind(&account.person_id)
            .bind(forgotten_id)
            .execute(&mut *transaction)
            .await?;
            result.imported_forgotten_ids += inserted.rows_affected() as usize;
        }
        transaction.commit().await?;
        Ok(result)
    }

    pub async fn lifecycle_summary(
        &self,
        account_token: &str,
    ) -> Result<LifecycleSummary, PostgresError> {
        let account = self.authenticate_account(account_token).await?;
        let row = sqlx::query(
            "SELECT
                (SELECT COUNT(*) FROM cairn_observations
                 WHERE person_id = $1 AND lifecycle->>'retention' = 'temporary') AS temporary_observations,
                (SELECT COUNT(*) FROM cairn_memories
                 WHERE person_id = $1 AND lifecycle->>'retention' = 'durable') AS durable_memories,
                ((SELECT COUNT(*) FROM cairn_observations
                  WHERE person_id = $1 AND lifecycle->>'sensitivity' IN ('sensitive', 'secret'))
                 + (SELECT COUNT(*) FROM cairn_memories
                    WHERE person_id = $1 AND lifecycle->>'sensitivity' IN ('sensitive', 'secret'))) AS sensitive_items,
                ((SELECT COUNT(*) FROM cairn_observations
                  WHERE person_id = $1 AND lifecycle->>'expiresAt' IS NOT NULL
                    AND (lifecycle->>'expiresAt')::bigint <= EXTRACT(EPOCH FROM now())::bigint)
                 + (SELECT COUNT(*) FROM cairn_memories
                    WHERE person_id = $1 AND lifecycle->>'expiresAt' IS NOT NULL
                    AND (lifecycle->>'expiresAt')::bigint <= EXTRACT(EPOCH FROM now())::bigint)) AS expired_items",
        )
        .bind(&account.person_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(LifecycleSummary {
            temporary_observations: row.try_get::<i64, _>("temporary_observations")? as usize,
            durable_memories: row.try_get::<i64, _>("durable_memories")? as usize,
            sensitive_items: row.try_get::<i64, _>("sensitive_items")? as usize,
            expired_items: row.try_get::<i64, _>("expired_items")? as usize,
        })
    }

    pub async fn audit_events(
        &self,
        account_token: &str,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, PostgresError> {
        let account = self.authenticate_session(account_token).await?;
        let limit = i64::try_from(limit.min(MAX_SEARCH_RESULTS as usize))
            .map_err(|_| PostgresError::Validation("limit exceeds PostgreSQL range".to_owned()))?;
        let rows = sqlx::query(
            "SELECT audit_id, action, subject_id, actor_device_id, metadata,
                    EXTRACT(EPOCH FROM created_at)::bigint::text AS created_at
             FROM cairn_audit_events
             WHERE person_id = $1 ORDER BY created_at DESC, audit_id DESC LIMIT $2",
        )
        .bind(&account.person_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(AuditEvent {
                    audit_id: row.try_get("audit_id")?,
                    action: row.try_get("action")?,
                    subject_id: row.try_get("subject_id")?,
                    actor_device_id: row.try_get("actor_device_id")?,
                    metadata: row.try_get::<Json<serde_json::Value>, _>("metadata")?.0,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect()
    }

    async fn all_memories_for_person(&self, person_id: &str) -> Result<Vec<Memory>, PostgresError> {
        let query = format!(
            "{} WHERE m.person_id = $1
              AND NOT EXISTS (SELECT 1 FROM cairn_forgotten_items f
                              WHERE f.person_id = m.person_id
                                AND (f.item_id = m.memory_id OR f.item_id IN
                                     (SELECT jsonb_array_elements_text(m.source_event_ids))))
              AND (m.lifecycle->>'expiresAt' IS NULL OR
                   (m.lifecycle->>'expiresAt')::bigint > EXTRACT(EPOCH FROM now())::bigint)
              AND {}
              ORDER BY m.created_at DESC, m.memory_id",
            memory_select_sql(),
            not_superseded_by_live_memory_sql(false)
        );
        let rows = sqlx::query(&query)
            .bind(person_id)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(memory_from_row).collect()
    }

    async fn record_audit_event(
        &self,
        person_id: &str,
        actor_device_id: Option<&str>,
        action: &str,
        subject_id: Option<&str>,
        metadata: serde_json::Value,
    ) -> Result<(), PostgresError> {
        sqlx::query(
            "INSERT INTO cairn_audit_events
                (audit_id, person_id, actor_device_id, action, subject_id, metadata)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(format!(
            "audit-{}",
            &cairn_core::hash_token(&random_token("audit"))[..24]
        ))
        .bind(person_id)
        .bind(actor_device_id)
        .bind(action)
        .bind(subject_id)
        .bind(Json(metadata))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn register_device(
        &self,
        account_token: &str,
        device_id: &str,
        person_id: &str,
        agent_id: Option<&str>,
        name: &str,
    ) -> Result<DeviceRegistration, PostgresError> {
        let account = self.authenticate_session(account_token).await?;
        if account.person_id != person_id {
            return Err(PostgresError::Validation(
                "account does not own the requested person".to_owned(),
            ));
        }
        if device_id.trim().is_empty() || name.trim().is_empty() {
            return Err(PostgresError::Validation(
                "device_id and device name must not be empty".to_owned(),
            ));
        }
        let token = random_token("cairn-device");
        let token_hash = cairn_core::hash_token(&token);
        let mut transaction = self.pool.begin().await?;
        let existing = sqlx::query(
            "SELECT person_id, status FROM cairn_devices WHERE device_id = $1 FOR UPDATE",
        )
        .bind(device_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(existing) = existing {
            let existing_person: String = existing.try_get("person_id")?;
            if existing_person != person_id {
                transaction.rollback().await?;
                return Err(PostgresError::Validation(format!(
                    "device already belongs to another person: {device_id}"
                )));
            }
            let existing_status: String = existing.try_get("status")?;
            if existing_status == "revoked" {
                transaction.rollback().await?;
                return Err(PostgresError::Validation(
                    "device is revoked; register a new device id".to_owned(),
                ));
            }
            sqlx::query(
                "UPDATE cairn_devices SET token_hash = $1, last_seen_at = now()
                 WHERE device_id = $2",
            )
            .bind(token_hash)
            .bind(device_id)
            .execute(&mut *transaction)
            .await?;
            record_audit(
                &mut transaction,
                person_id,
                None,
                "device_resumed",
                Some(device_id),
                serde_json::json!({ "agent_id": agent_id }),
            )
            .await?;
            transaction.commit().await?;
            return Ok(DeviceRegistration {
                device: self.device_by_id(device_id).await?,
                token,
            });
        }
        sqlx::query(
            "INSERT INTO cairn_devices (device_id, person_id, agent_id, name, token_hash)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(device_id)
        .bind(person_id)
        .bind(agent_id)
        .bind(name)
        .bind(token_hash)
        .execute(&mut *transaction)
        .await?;
        record_audit(
            &mut transaction,
            person_id,
            None,
            "device_registered",
            Some(device_id),
            serde_json::json!({ "agent_id": agent_id }),
        )
        .await?;
        transaction.commit().await?;
        let now = now_seconds().to_string();
        Ok(DeviceRegistration {
            device: Device {
                device_id: device_id.to_owned(),
                person_id: person_id.to_owned(),
                agent_id: agent_id.map(ToOwned::to_owned),
                name: name.to_owned(),
                status: DeviceStatus::Active,
                registered_at: now.clone(),
                last_seen_at: now,
                cursor: 0,
            },
            token,
        })
    }

    pub async fn list_devices(&self, account_token: &str) -> Result<Vec<Device>, PostgresError> {
        let account = self.authenticate_session(account_token).await?;
        let query = format!(
            "{} WHERE d.person_id = $1 ORDER BY d.registered_at",
            device_select_columns()
        );
        let rows = sqlx::query(&query)
            .bind(account.person_id)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(device_from_row).collect()
    }

    pub async fn revoke_device(
        &self,
        account_token: &str,
        device_id: &str,
    ) -> Result<Device, PostgresError> {
        let account = self.authenticate_session(account_token).await?;
        let result = sqlx::query(
            "UPDATE cairn_devices SET status = 'revoked'
             WHERE device_id = $1 AND person_id = $2",
        )
        .bind(device_id)
        .bind(&account.person_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(PostgresError::Validation(
                "unknown device or device is outside the caller scope".to_owned(),
            ));
        }
        self.record_audit_event(
            &account.person_id,
            None,
            "device_revoked",
            Some(device_id),
            serde_json::json!({}),
        )
        .await?;
        self.device_by_id(device_id).await
    }

    pub async fn rotate_device(
        &self,
        account_token: &str,
        device_id: &str,
    ) -> Result<DeviceRegistration, PostgresError> {
        let account = self.authenticate_session(account_token).await?;
        let token = random_token("cairn-device");
        let token_hash = cairn_core::hash_token(&token);
        let result = sqlx::query(
            "UPDATE cairn_devices SET token_hash = $1, last_seen_at = now()
             WHERE device_id = $2 AND person_id = $3 AND status = 'active'",
        )
        .bind(token_hash)
        .bind(device_id)
        .bind(&account.person_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(PostgresError::Validation(
                "unknown, revoked, or out-of-scope device".to_owned(),
            ));
        }
        self.record_audit_event(
            &account.person_id,
            None,
            "device_rotated",
            Some(device_id),
            serde_json::json!({}),
        )
        .await?;
        Ok(DeviceRegistration {
            device: self.device_by_id(device_id).await?,
            token,
        })
    }

    pub async fn sync_push(
        &self,
        device_token: &str,
        input: ObservationInput,
    ) -> Result<SyncPushResult, PostgresError> {
        let device = self.authenticate_device(device_token).await?;
        let mut input = input;
        let mut transaction = self.pool.begin().await?;
        if input.context == WorkContext::default() {
            // Copy only this person's stored session. A missing row stays empty
            // and is inserted below so the observation foreign key holds.
            if let Some(row) = sqlx::query(
                "SELECT context FROM cairn_sessions
                 WHERE person_id = $1 AND session_id = $2
                 FOR UPDATE",
            )
            .bind(&device.person_id)
            .bind(&input.session_id)
            .fetch_optional(&mut *transaction)
            .await?
            {
                input.context = row.try_get::<Json<WorkContext>, _>("context")?.0;
            }
        }
        let observation = normalize_observation(&input)?;
        let lifecycle = serde_json::to_value(&observation.lifecycle)?;
        let context = serde_json::to_value(&observation.context)?;
        let event_query = sync_select_event_sql();
        if let Some(row) = sqlx::query(&event_query)
            .bind(&device.person_id)
            .bind(&observation.event_id)
            .fetch_optional(&mut *transaction)
            .await?
        {
            let event = sync_event_from_row(&row)?;
            let stored = &event.observation;
            if (
                &stored.session_id,
                &stored.content,
                &stored.role,
                &stored.source,
            ) != (
                &observation.session_id,
                &observation.content,
                &observation.role,
                &observation.source,
            ) {
                transaction.rollback().await?;
                return Err(PostgresError::Conflict(format!(
                    "event id already used with different content: {}",
                    observation.event_id
                )));
            }
            // A pure replay changes nothing but the device's last-seen time; no audit row.
            sqlx::query(
                "UPDATE cairn_devices SET last_seen_at = now()
                 WHERE device_id = $1 AND last_seen_at < now() - interval '60 seconds'",
            )
            .bind(&device.device_id)
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await?;
            return Ok(SyncPushResult {
                accepted: true,
                duplicate: true,
                cursor: event.cursor,
                observation: event.observation,
            });
        }
        // An observation with no sync event (an account capture) is adopted when the push is
        // the same event. The insert below then keeps the stored row and adds the sync event.
        if let Some(row) = sqlx::query(
            "SELECT session_id, content, role, source FROM cairn_observations
             WHERE person_id = $1 AND event_id = $2",
        )
        .bind(&device.person_id)
        .bind(&observation.event_id)
        .fetch_optional(&mut *transaction)
        .await?
        {
            let same = row.try_get::<String, _>("session_id")? == observation.session_id
                && row.try_get::<String, _>("content")? == observation.content
                && row.try_get::<String, _>("role")? == observation.role
                && row.try_get::<String, _>("source")? == observation.source;
            if !same {
                transaction.rollback().await?;
                return Err(PostgresError::Conflict(format!(
                    "event id already used with different content: {}",
                    observation.event_id
                )));
            }
        }
        // A new row takes the pushed context. An empty row takes the first real one (the
        // `start_session` rule); a real context is never replaced.
        let empty_context = serde_json::to_value(WorkContext::default())?;
        sqlx::query(
            "INSERT INTO cairn_sessions (session_id, person_id, context)
             VALUES ($1, $2, $3)
             ON CONFLICT (person_id, session_id) DO UPDATE
             SET context = EXCLUDED.context
             WHERE ($4::jsonb || cairn_sessions.context) = $4::jsonb
               AND EXCLUDED.context <> $4::jsonb",
        )
        .bind(&observation.session_id)
        .bind(&device.person_id)
        .bind(context.clone())
        .bind(&empty_context)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO cairn_observations
                (person_id, event_id, session_id, content, role, source, importance,
                 lifecycle, scope, context, captured_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, to_timestamp($11::double precision))
             ON CONFLICT (person_id, event_id) DO NOTHING",
        )
        .bind(&device.person_id)
        .bind(&observation.event_id)
        .bind(&observation.session_id)
        .bind(&observation.content)
        .bind(&observation.role)
        .bind(&observation.source)
        .bind(&observation.importance)
        .bind(lifecycle)
        .bind(scope_name(&observation.scope))
        .bind(context)
        .bind(
            observation
                .captured_at
                .parse::<i64>()
                .unwrap_or_else(|_| now_seconds() as i64),
        )
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE cairn_sessions SET pending_since = now()
             WHERE person_id = $1 AND session_id = $2",
        )
        .bind(&device.person_id)
        .bind(&observation.session_id)
        .execute(&mut *transaction)
        .await?;
        lock_sync_cursor(&mut transaction, &device.person_id).await?;
        let sync_insert = sqlx::query(
            "INSERT INTO cairn_sync_events (person_id, event_id, device_id)
             VALUES ($1, $2, $3) ON CONFLICT (person_id, event_id) DO NOTHING",
        )
        .bind(&device.person_id)
        .bind(&observation.event_id)
        .bind(&device.device_id)
        .execute(&mut *transaction)
        .await?;
        let row = sqlx::query(&event_query)
            .bind(&device.person_id)
            .bind(&observation.event_id)
            .fetch_one(&mut *transaction)
            .await?;
        sqlx::query(
            "UPDATE cairn_devices SET last_seen_at = now()
                 WHERE device_id = $1 AND last_seen_at < now() - interval '60 seconds'",
        )
        .bind(&device.device_id)
        .execute(&mut *transaction)
        .await?;
        record_audit(
            &mut transaction,
            &device.person_id,
            Some(&device.device_id),
            "sync_push",
            Some(&observation.event_id),
            serde_json::json!({ "cursor": row.try_get::<i64, _>("cursor")? }),
        )
        .await?;
        transaction.commit().await?;
        let event = sync_event_from_row(&row)?;
        let duplicate = sync_insert.rows_affected() == 0;
        if !duplicate {
            // The push is acknowledged. Housekeeping failures must not turn it into an error.
            if let Err(error) = self
                .auto_register_project(&device.person_id, &event.observation.context)
                .await
            {
                eprintln!("cairn: automatic project registration failed: {error}");
            }
            self.finish_idle_sessions(&device.person_id).await;
        }
        Ok(SyncPushResult {
            accepted: true,
            duplicate,
            cursor: event.cursor,
            observation: event.observation,
        })
    }

    pub async fn sync_pull(
        &self,
        device_token: &str,
        cursor: u64,
    ) -> Result<SyncPullResult, PostgresError> {
        let device = self.authenticate_device(device_token).await?;
        let cursor_param = database_cursor(cursor)?;
        // One snapshot for both lists, so a commit between the two reads cannot put an item
        // below the page cursor that the first read did not see.
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *transaction)
            .await?;
        let query = format!(
            "{} AND se.cursor > $2
               AND NOT EXISTS (
                   SELECT 1 FROM cairn_forgotten_items f
                   WHERE f.person_id = se.person_id AND f.item_id = se.event_id
               )
             ORDER BY se.cursor LIMIT {MAX_PULL_EVENTS}",
            sync_select_sql()
        );
        let rows = sqlx::query(&query)
            .bind(&device.person_id)
            .bind(cursor_param)
            .fetch_all(&mut *transaction)
            .await?;
        let events = rows
            .iter()
            .map(sync_event_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let deletion_rows = sqlx::query(&format!(
            "SELECT cursor, item_id FROM cairn_sync_deletions
             WHERE person_id = $1 AND cursor > $2
             ORDER BY cursor LIMIT {MAX_PULL_EVENTS}"
        ))
        .bind(&device.person_id)
        .bind(cursor_param)
        .fetch_all(&mut *transaction)
        .await?;
        let deletions = deletion_rows
            .iter()
            .map(|row| {
                let deletion_cursor: i64 = row.try_get("cursor")?;
                Ok(SyncDeletion {
                    cursor: deletion_cursor.max(0) as u64,
                    item_id: row.try_get("item_id")?,
                })
            })
            .collect::<Result<Vec<_>, PostgresError>>()?;
        let mut events = events;
        let mut deletions = deletions;
        let next_cursor = pull_page_end(
            events.last().map(|event| event.cursor),
            events.len(),
            deletions.last().map(|deletion| deletion.cursor),
            deletions.len(),
            cursor.max(device.cursor),
        );
        events.retain(|event| event.cursor <= next_cursor);
        deletions.retain(|deletion| deletion.cursor <= next_cursor);
        sqlx::query(
            "UPDATE cairn_devices SET pull_cursor = GREATEST(pull_cursor, $1), last_seen_at = now()
             WHERE device_id = $2
               AND (pull_cursor < $1 OR last_seen_at < now() - interval '60 seconds')",
        )
        .bind(next_cursor as i64)
        .bind(&device.device_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(SyncPullResult {
            cursor: next_cursor,
            events,
            deletions,
        })
    }

    pub async fn sync_diagnostics(
        &self,
        device_token: &str,
    ) -> Result<SyncDiagnostics, PostgresError> {
        let device = self.authenticate_device(device_token).await?;
        let server_cursor = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(MAX(cursor), 0) FROM cairn_sync_events WHERE person_id = $1",
        )
        .bind(&device.person_id)
        .fetch_one(&self.pool)
        .await?
        .max(0) as u64;
        let lag = server_cursor.saturating_sub(device.cursor);
        Ok(SyncDiagnostics {
            device_id: device.device_id,
            device_cursor: device.cursor,
            server_cursor,
            lag,
            pending_events: 0,
            rejected_events: 0,
            status: if lag == 0 { "caught_up" } else { "behind" }.to_owned(),
            recovery_hint: (lag > 0).then(|| {
                "Pull from the last persisted cursor to resume synchronization.".to_owned()
            }),
        })
    }

    async fn authenticate_device(&self, token: &str) -> Result<Device, PostgresError> {
        let token_hash = cairn_core::hash_token(token);
        let query = device_select_sql();
        let row = sqlx::query(&query)
            .bind(token_hash)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| PostgresError::Validation("device authentication failed".to_owned()))?;
        let device = device_from_row(&row)?;
        if device.status != DeviceStatus::Active {
            return Err(PostgresError::Validation("device is revoked".to_owned()));
        }
        if !row.try_get::<bool, _>("account_active")? {
            return Err(PostgresError::Validation(
                "account authentication failed".to_owned(),
            ));
        }
        Ok(device)
    }

    async fn device_by_id(&self, device_id: &str) -> Result<Device, PostgresError> {
        let query = format!("{} WHERE d.device_id = $1", device_select_columns());
        let row = sqlx::query(&query)
            .bind(device_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| PostgresError::Validation(format!("unknown device: {device_id}")))?;
        device_from_row(&row)
    }
}

async fn insert_auth_session(
    transaction: &mut Transaction<'_, Postgres>,
    session_id: &str,
    person_id: &str,
    token_hash: &str,
    expires_at: u64,
) -> Result<(), PostgresError> {
    sqlx::query(
        "INSERT INTO cairn_auth_sessions (session_id, person_id, token_hash, expires_at)
         VALUES ($1, $2, $3, to_timestamp($4::double precision))",
    )
    .bind(session_id)
    .bind(person_id)
    .bind(token_hash)
    .bind(expires_at as i64)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn record_audit(
    transaction: &mut Transaction<'_, Postgres>,
    person_id: &str,
    actor_device_id: Option<&str>,
    action: &str,
    subject_id: Option<&str>,
    metadata: serde_json::Value,
) -> Result<(), PostgresError> {
    sqlx::query(
        "INSERT INTO cairn_audit_events
            (audit_id, person_id, actor_device_id, action, subject_id, metadata)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(format!(
        "audit-{}",
        &cairn_core::hash_token(&random_token("audit"))[..24]
    ))
    .bind(person_id)
    .bind(actor_device_id)
    .bind(action)
    .bind(subject_id)
    .bind(Json(metadata))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn insert_memory(
    transaction: &mut Transaction<'_, Postgres>,
    person_id: &str,
    memory: &Memory,
) -> Result<(), PostgresError> {
    let source_event_ids = serde_json::to_value(&memory.source_event_ids)?;
    let context = serde_json::to_value(&memory.context)?;
    let lifecycle = serde_json::to_value(&memory.lifecycle)?;
    let next_steps = serde_json::to_value(&memory.next_steps)?;
    let unresolved_work = serde_json::to_value(&memory.unresolved_work)?;
    sqlx::query(
        "INSERT INTO cairn_memories
            (memory_id, person_id, session_id, content, source_event_ids, context,
             lifecycle, kind, scope, project_id, next_steps, unresolved_work,
             supersedes, promoted_from, promotion_reason, verified_commit, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                  to_timestamp($17::double precision))",
    )
    .bind(&memory.memory_id)
    .bind(person_id)
    .bind(&memory.session_id)
    .bind(&memory.content)
    .bind(source_event_ids)
    .bind(context)
    .bind(lifecycle)
    .bind(memory_kind_name(&memory.kind))
    .bind(scope_name(&memory.scope))
    .bind(&memory.project_id)
    .bind(next_steps)
    .bind(unresolved_work)
    .bind(&memory.supersedes)
    .bind(&memory.promoted_from)
    .bind(&memory.promotion_reason)
    .bind(&memory.verified_commit)
    .bind(
        memory
            .created_at
            .parse::<i64>()
            .unwrap_or_else(|_| now_seconds() as i64),
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn index_project_memory_tx(
    transaction: &mut Transaction<'_, Postgres>,
    person_id: &str,
    project_id: &str,
    memory: &Memory,
) -> Result<(), PostgresError> {
    let mut entities = vec![("decision", memory.memory_id.clone(), None)];
    entities.extend(
        memory
            .context
            .files
            .iter()
            .cloned()
            .map(|value| ("file", value, None)),
    );
    entities.extend(
        memory
            .context
            .symbols
            .iter()
            .cloned()
            .map(|value| ("symbol", value, None)),
    );
    entities.extend(
        memory
            .context
            .handoffs
            .iter()
            .cloned()
            .map(|value| ("handoff", value, Some(memory.content.clone()))),
    );
    entities.extend(
        memory
            .context
            .runbooks
            .iter()
            .cloned()
            .map(|value| ("runbook", value, Some(memory.content.clone()))),
    );
    for (kind, key, content) in entities {
        sqlx::query(
            "INSERT INTO cairn_project_entities
                (person_id, entity_id, project_id, kind, entity_key, content,
                 source_memory_id, commit_sha)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (person_id, project_id, kind, entity_key) DO NOTHING",
        )
        .bind(person_id)
        .bind(format!(
            "entity-{}",
            &cairn_core::hash_token(&random_token("entity"))[..24]
        ))
        .bind(project_id)
        .bind(kind)
        .bind(key)
        .bind(content.map(|value| redact_sensitive_content(value.as_str())))
        .bind(&memory.memory_id)
        .bind(&memory.verified_commit)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

struct FreshnessEventInsert<'a> {
    project_id: &'a str,
    memory_id: &'a str,
    previous_commit: Option<String>,
    commit: &'a str,
    action: &'a str,
    reason: Option<String>,
}

async fn insert_freshness_event_tx(
    transaction: &mut Transaction<'_, Postgres>,
    person_id: &str,
    event: FreshnessEventInsert<'_>,
) -> Result<(), PostgresError> {
    sqlx::query(
        "INSERT INTO cairn_project_freshness_events
            (person_id, event_id, project_id, memory_id, previous_commit,
             commit_sha, action, reason)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(person_id)
    .bind(format!(
        "freshness-{}",
        &cairn_core::hash_token(&random_token("freshness"))[..24]
    ))
    .bind(event.project_id)
    .bind(event.memory_id)
    .bind(event.previous_commit)
    .bind(event.commit)
    .bind(event.action)
    .bind(event.reason)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn insert_observation(
    transaction: &mut Transaction<'_, Postgres>,
    person_id: &str,
    observation: &Observation,
) -> Result<(), PostgresError> {
    let lifecycle = serde_json::to_value(&observation.lifecycle)?;
    let context = serde_json::to_value(&observation.context)?;
    sqlx::query(
        "INSERT INTO cairn_observations
            (person_id, event_id, session_id, content, role, source, importance,
             lifecycle, scope, context, captured_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                 to_timestamp($11::double precision))",
    )
    .bind(person_id)
    .bind(&observation.event_id)
    .bind(&observation.session_id)
    .bind(&observation.content)
    .bind(&observation.role)
    .bind(&observation.source)
    .bind(&observation.importance)
    .bind(lifecycle)
    .bind(scope_name(&observation.scope))
    .bind(context)
    .bind(
        observation
            .captured_at
            .parse::<i64>()
            .unwrap_or_else(|_| now_seconds() as i64),
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// The summary is current as of this transaction. A capture committed after it read the
/// observations sets the flag again.
async fn clear_session_pending(
    transaction: &mut Transaction<'_, Postgres>,
    person_id: &str,
    session_id: &str,
) -> Result<(), PostgresError> {
    sqlx::query(
        "UPDATE cairn_sessions SET pending_since = NULL
         WHERE person_id = $1 AND session_id = $2 AND pending_since <= now()",
    )
    .bind(person_id)
    .bind(session_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Serialises cursor allocation with commit for one person. A cursor is allocated when its
/// row is inserted but becomes visible only at commit, so without this a later cursor could
/// commit first and a pull would move past an earlier one that commits afterwards. With it,
/// this person's cursors commit in allocation order, so everything a pull sees is a prefix.
/// The lock is released with the transaction.
async fn lock_sync_cursor(
    transaction: &mut Transaction<'_, Postgres>,
    person_id: &str,
) -> Result<(), PostgresError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('cairn-sync-cursor:' || $1, 0))")
        .bind(person_id)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn insert_sync_deletion(
    transaction: &mut Transaction<'_, Postgres>,
    person_id: &str,
    item_id: &str,
) -> Result<(), PostgresError> {
    lock_sync_cursor(transaction, person_id).await?;
    sqlx::query(
        "INSERT INTO cairn_sync_deletions (person_id, item_id, cursor)
         SELECT $1, $2, nextval(pg_get_serial_sequence('cairn_sync_events', 'cursor'))
         WHERE NOT EXISTS (
             SELECT 1 FROM cairn_sync_deletions
             WHERE person_id = $1 AND item_id = $2
         )",
    )
    .bind(person_id)
    .bind(item_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn insert_forget(
    transaction: &mut Transaction<'_, Postgres>,
    person_id: &str,
    item_id: &str,
) -> Result<(), PostgresError> {
    sqlx::query(
        "INSERT INTO cairn_forgotten_items (person_id, item_id)
         VALUES ($1, $2) ON CONFLICT (person_id, item_id) DO NOTHING",
    )
    .bind(person_id)
    .bind(item_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn session_from_row(row: &PgRow) -> Result<Session, PostgresError> {
    Ok(Session {
        session_id: row.try_get("session_id")?,
        // PostgreSQL identity stays in the person_id query. The file field is absent here.
        person_id: None,
        context: row.try_get::<Json<WorkContext>, _>("context")?.0,
        started_at: row.try_get("started_at")?,
        ended_at: row.try_get("ended_at")?,
    })
}

fn not_superseded_by_live_memory_sql(same_project: bool) -> String {
    let project_clause = if same_project {
        "AND newer.project_id = m.project_id"
    } else {
        ""
    };
    // A live correction of the personal origin also hides its promoted copies (ADR 0010).
    format!(
        "NOT EXISTS (
             SELECT 1 FROM cairn_memories newer
             WHERE newer.person_id = m.person_id
               AND ((newer.supersedes = m.memory_id {project_clause})
                    OR newer.supersedes = m.promoted_from)
               AND NOT EXISTS (
                   SELECT 1 FROM cairn_forgotten_items f
                   WHERE f.person_id = newer.person_id
                     AND (f.item_id = newer.memory_id OR f.item_id IN
                          (SELECT jsonb_array_elements_text(newer.source_event_ids)))
               )
               AND (newer.lifecycle->>'expiresAt' IS NULL OR
                    (newer.lifecycle->>'expiresAt')::bigint > EXTRACT(EPOCH FROM now())::bigint)
         )"
    )
}

/// The earlier project copy of the memory a promoted correction supersedes. The copy links
/// to it instead of to the personal id, which a member must never see.
async fn prior_project_copy(
    transaction: &mut Transaction<'_, Postgres>,
    person_id: &str,
    project_id: &str,
    corrected: Option<&str>,
) -> Result<Option<String>, PostgresError> {
    let Some(corrected) = corrected else {
        return Ok(None);
    };
    Ok(sqlx::query_scalar::<_, String>(&format!(
        "SELECT m.memory_id FROM cairn_memories m
             WHERE m.person_id = $1 AND m.project_id = $2 AND m.promoted_from = $3 AND {}
             ORDER BY m.created_at LIMIT 1",
        live_memory_sql()
    ))
    .bind(person_id)
    .bind(project_id)
    .bind(corrected)
    .fetch_optional(&mut **transaction)
    .await?)
}

/// Predicate on an aliased memory `m`: not forgotten and not expired. A forgotten copy is a
/// tombstone, so promoting again creates a new copy.
fn live_memory_sql() -> &'static str {
    "NOT EXISTS (SELECT 1 FROM cairn_forgotten_items f
                 WHERE f.person_id = m.person_id AND f.item_id = m.memory_id)
     AND (m.lifecycle->>'expiresAt' IS NULL OR
          (m.lifecycle->>'expiresAt')::bigint > EXTRACT(EPOCH FROM now())::bigint)"
}

fn session_memory_select_sql() -> String {
    format!(
        "{} WHERE m.person_id = $1 AND m.memory_id = $2",
        memory_select_sql()
    )
}

fn memory_select_sql() -> &'static str {
    "SELECT m.memory_id, m.person_id, m.session_id, m.content,
            m.source_event_ids, m.context, m.lifecycle, m.kind, m.scope,
            m.project_id, m.next_steps, m.unresolved_work, m.supersedes,
            m.promoted_from, m.promotion_reason,
            m.verified_commit,
            EXTRACT(EPOCH FROM m.created_at)::bigint::text AS created_at
     FROM cairn_memories m"
}

fn shared_memory_select_sql() -> &'static str {
    "SELECT s.share_reason, m.memory_id, m.person_id, m.session_id, m.content,
            m.source_event_ids, m.context, m.lifecycle, m.kind, m.scope,
            m.project_id, m.next_steps, m.unresolved_work, m.supersedes,
            m.promoted_from, m.promotion_reason,
            m.verified_commit,
            EXTRACT(EPOCH FROM m.created_at)::bigint::text AS created_at
     FROM cairn_memories m"
}

/// The eligibility, query, and filter predicate shared by every PostgreSQL search path.
/// `$1` is the person, `$2` the query; filter binds follow.
fn search_conditions(
    person_id: &str,
    query: &str,
    filter: &RecallFilter,
) -> (Vec<String>, Vec<String>) {
    let mut binds = vec![person_id.to_owned(), query.to_owned()];
    let mut conditions = vec![
        "m.person_id = $1".to_owned(),
        "m.search_tsv @@ plainto_tsquery('simple', $2)".to_owned(),
        "NOT EXISTS (
             SELECT 1 FROM cairn_forgotten_items f
             WHERE f.person_id = m.person_id
               AND (f.item_id = m.memory_id OR f.item_id IN
                    (SELECT jsonb_array_elements_text(m.source_event_ids)))
         )"
        .to_owned(),
        "(m.lifecycle->>'expiresAt' IS NULL OR
          (m.lifecycle->>'expiresAt')::bigint > EXTRACT(EPOCH FROM now())::bigint)"
            .to_owned(),
        not_superseded_by_live_memory_sql(false),
    ];
    append_filter_conditions(filter, &mut conditions, &mut binds);
    (conditions, binds)
}

/// Packs a capped search window. `omitted_count` is every eligible match not returned,
/// including matches past `MAX_SEARCH_RESULTS`; `omitted_memory_ids` lists only the window.
fn pack_window(
    query: String,
    results: &mut Vec<RecallResult>,
    limit: usize,
    total: usize,
) -> ContextPack {
    let mut pack = build_context_pack(query, results, limit);
    pack.omitted_count = total.saturating_sub(pack.results.len());
    if pack.omitted_count > pack.omitted_memory_ids.len() {
        pack.recovery_hint = Some(format!(
            "Results are capped at {MAX_SEARCH_RESULTS} matches, so omitted_memory_ids lists only part of the omitted results and a larger limit cannot recover the rest. Narrow the query or filter."
        ));
    }
    pack
}

fn memory_search_select_sql() -> String {
    format!(
        "{} , ts_rank_cd(m.search_tsv, plainto_tsquery('simple', $2)) AS search_score,
         COUNT(*) OVER () AS total_matches
         FROM cairn_memories m",
        memory_select_columns()
    )
}

fn memory_select_columns() -> &'static str {
    "SELECT m.memory_id, m.person_id, m.session_id, m.content,
            m.source_event_ids, m.context, m.lifecycle, m.kind, m.scope,
            m.project_id, m.next_steps, m.unresolved_work, m.supersedes,
            m.promoted_from, m.promotion_reason,
            m.verified_commit,
            EXTRACT(EPOCH FROM m.created_at)::bigint::text AS created_at"
}

fn observation_select_sql() -> &'static str {
    "SELECT o.event_id AS observation_event_id,
            o.session_id AS observation_session_id,
            o.content AS observation_content,
            o.role AS observation_role,
            o.source AS observation_source,
            o.importance AS observation_importance,
            o.lifecycle AS observation_lifecycle,
            o.scope AS observation_scope,
            o.context AS observation_context,
            EXTRACT(EPOCH FROM o.captured_at)::bigint::text AS observation_captured_at
     FROM cairn_observations o"
}

fn shared_memory_from_row(row: &PgRow) -> Result<SharedProjectMemory, PostgresError> {
    let memory = memory_from_row(row)?;
    Ok(SharedProjectMemory {
        share_reason: row.try_get("share_reason")?,
        // A member never sees the personal memory the copy came from.
        memory: Memory {
            promoted_from: None,
            ..memory
        },
    })
}

fn memory_from_row(row: &PgRow) -> Result<Memory, PostgresError> {
    Ok(Memory {
        memory_id: row.try_get("memory_id")?,
        session_id: row.try_get("session_id")?,
        content: row.try_get("content")?,
        source_event_ids: row.try_get::<Json<Vec<String>>, _>("source_event_ids")?.0,
        context: row.try_get::<Json<WorkContext>, _>("context")?.0,
        lifecycle: row.try_get::<Json<Lifecycle>, _>("lifecycle")?.0,
        kind: parse_memory_kind(row.try_get("kind")?)?,
        scope: parse_scope(row.try_get("scope")?)?,
        project_id: row.try_get("project_id")?,
        next_steps: row.try_get::<Json<Vec<String>>, _>("next_steps")?.0,
        unresolved_work: row.try_get::<Json<Vec<String>>, _>("unresolved_work")?.0,
        supersedes: row.try_get("supersedes")?,
        promoted_from: row.try_get("promoted_from")?,
        promotion_reason: row.try_get("promotion_reason")?,
        verified_commit: row.try_get("verified_commit")?,
        created_at: row.try_get("created_at")?,
    })
}

fn append_filter_conditions(
    filter: &RecallFilter,
    conditions: &mut Vec<String>,
    binds: &mut Vec<String>,
) {
    if filter.session_id.is_none() && filter.context.is_none() {
        return;
    }
    let mut alternatives = vec!["m.scope = 'global'".to_owned()];
    if let Some(session_id) = &filter.session_id {
        binds.push(session_id.clone());
        alternatives.push(format!(
            "(m.scope = 'session' AND m.session_id = ${})",
            binds.len()
        ));
    }
    if let Some(context) = &filter.context
        && let Some(repository) = &context.repository
    {
        binds.push(repository.clone());
        let repository_bind = binds.len();
        alternatives.push(format!(
            "(m.scope = 'project' AND m.context->>'repository' = ${repository_bind})"
        ));
        if let Some(branch) = &context.branch {
            binds.push(branch.clone());
            let branch_bind = binds.len();
            alternatives.push(format!(
                "(m.scope = 'branch' AND m.context->>'repository' = ${repository_bind} AND m.context->>'branch' = ${branch_bind})"
            ));
            if let Some(worktree) = &context.worktree {
                binds.push(worktree.clone());
                let worktree_bind = binds.len();
                alternatives.push(format!(
                    "(m.scope = 'worktree' AND m.context->>'repository' = ${repository_bind} AND m.context->>'branch' = ${branch_bind} AND m.context->>'worktree' = ${worktree_bind})"
                ));
            }
        }
    }
    conditions.push(format!("({})", alternatives.join(" OR ")));
}

fn import_conflict(kind: &str, id: &str) -> PostgresError {
    PostgresError::Validation(format!("import conflicts with existing {kind}: {id}"))
}

fn team_member_from_row(row: &PgRow) -> Result<ProjectTeamMember, PostgresError> {
    Ok(ProjectTeamMember {
        person_id: row.try_get("member_person_id")?,
        email: row.try_get("email")?,
        created_at: row.try_get("created_at")?,
    })
}

fn membership_from_row(row: &PgRow) -> Result<ProjectMembership, PostgresError> {
    Ok(ProjectMembership {
        project_id: row.try_get("project_id")?,
        person_id: row.try_get("member_person_id")?,
        email: row.try_get("email")?,
        created_at: row.try_get("created_at")?,
    })
}

fn policy_name(policy: &ProjectSharePolicy) -> &'static str {
    match policy {
        ProjectSharePolicy::Manual => "manual",
        ProjectSharePolicy::AutoNonsensitiveProjectMemory => SHARE_REASON_AUTO,
    }
}

fn postgres_not_auto_shared_reason(policy: &ProjectSharePolicy, memory: &Memory) -> String {
    if *policy == ProjectSharePolicy::Manual {
        "space policy is manual".to_owned()
    } else if memory.lifecycle.sensitivity == cairn_core::Sensitivity::Sensitive {
        "memory is sensitive".to_owned()
    } else {
        "automatic policy did not match this memory".to_owned()
    }
}

async fn auto_share_project_memory_tx(
    transaction: &mut Transaction<'_, Postgres>,
    owner_person_id: &str,
    project_id: &str,
    memory: &Memory,
) -> Result<(), PostgresError> {
    if memory.scope != MemoryScope::Project
        || matches!(
            memory.lifecycle.sensitivity,
            cairn_core::Sensitivity::Secret | cairn_core::Sensitivity::Sensitive
        )
    {
        return Ok(());
    }
    let policy = sqlx::query_scalar::<_, String>(
        "SELECT policy FROM cairn_project_share_policies
         WHERE owner_person_id = $1 AND project_id = $2",
    )
    .bind(owner_person_id)
    .bind(project_id)
    .fetch_optional(&mut **transaction)
    .await?;
    if policy.as_deref() != Some(SHARE_REASON_AUTO) {
        return Ok(());
    }
    let members = sqlx::query_scalar::<_, String>(
        "SELECT member_person_id FROM cairn_project_members
         WHERE owner_person_id = $1 AND project_id = $2",
    )
    .bind(owner_person_id)
    .bind(project_id)
    .fetch_all(&mut **transaction)
    .await?;
    for member_person_id in members {
        let dismissed = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM cairn_share_candidate_dismissals
                WHERE owner_person_id = $1 AND project_id = $2
                  AND memory_id = $3 AND member_person_id = $4
             )",
        )
        .bind(owner_person_id)
        .bind(project_id)
        .bind(&memory.memory_id)
        .bind(&member_person_id)
        .fetch_one(&mut **transaction)
        .await?;
        if dismissed {
            continue;
        }
        let active = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM cairn_project_shares
                WHERE owner_person_id = $1 AND project_id = $2 AND memory_id = $3
                  AND member_person_id = $4 AND revoked_at IS NULL
             )",
        )
        .bind(owner_person_id)
        .bind(project_id)
        .bind(&memory.memory_id)
        .bind(&member_person_id)
        .fetch_one(&mut **transaction)
        .await?;
        if active {
            continue;
        }
        let reactivated = sqlx::query(
            "UPDATE cairn_project_shares
             SET revoked_at = NULL, share_reason = $5
             WHERE owner_person_id = $1 AND project_id = $2 AND memory_id = $3
               AND member_person_id = $4 AND revoked_at IS NOT NULL",
        )
        .bind(owner_person_id)
        .bind(project_id)
        .bind(&memory.memory_id)
        .bind(&member_person_id)
        .bind(SHARE_REASON_AUTO)
        .execute(&mut **transaction)
        .await?;
        if reactivated.rows_affected() == 0 {
            sqlx::query(
                "INSERT INTO cairn_project_shares
                    (owner_person_id, project_id, memory_id, member_person_id, share_reason)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(owner_person_id)
            .bind(project_id)
            .bind(&memory.memory_id)
            .bind(&member_person_id)
            .bind(SHARE_REASON_AUTO)
            .execute(&mut **transaction)
            .await?;
        }
        record_audit(
            transaction,
            owner_person_id,
            None,
            "project_memory_shared",
            Some(&memory.memory_id),
            serde_json::json!({
                "project_id": project_id,
                "member_person_id": member_person_id,
                "memory_id": memory.memory_id,
            }),
        )
        .await?;
    }
    Ok(())
}

fn share_from_row(row: &PgRow) -> Result<ProjectMemoryShare, PostgresError> {
    Ok(ProjectMemoryShare {
        project_id: row.try_get("project_id")?,
        memory_id: row.try_get("memory_id")?,
        member_person_id: row.try_get("member_person_id")?,
        member_email: None,
        created_at: row.try_get("created_at")?,
        revoked_at: row.try_get("revoked_at")?,
    })
}

fn account_from_row(row: &PgRow) -> Result<Account, PostgresError> {
    let status: String = row.try_get("status")?;
    Ok(Account {
        person_id: row.try_get("person_id")?,
        email: row.try_get("email")?,
        status: match status.as_str() {
            "active" => AccountStatus::Active,
            "disabled" => AccountStatus::Disabled,
            other => {
                return Err(PostgresError::Validation(format!(
                    "unknown account status: {other}"
                )));
            }
        },
        is_admin: row.try_get("is_admin")?,
        created_at: row.try_get("created_at")?,
    })
}

fn project_from_row(row: &PgRow) -> Result<Project, PostgresError> {
    Ok(Project {
        project_id: row.try_get("project_id")?,
        repository: row.try_get("repository")?,
        name: row.try_get("name")?,
        default_branch: row.try_get("default_branch")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn branch_select_sql(condition: &str) -> String {
    format!(
        "SELECT b.branch_id, b.project_id, b.name, b.base_branch, b.base_commit,
                b.head_commit, b.status, b.status_reason, b.merge_commit,
                b.merge_memory_id,
                EXTRACT(EPOCH FROM b.created_at)::bigint::text AS created_at,
                EXTRACT(EPOCH FROM b.updated_at)::bigint::text AS updated_at
         FROM cairn_project_branches b
         WHERE b.person_id = $1 AND {condition}
         ORDER BY b.name"
    )
}

fn worktree_select_sql(condition: &str) -> String {
    format!(
        "SELECT w.worktree_id, w.project_id, w.branch_id, w.path, w.commit_sha,
                w.status,
                EXTRACT(EPOCH FROM w.created_at)::bigint::text AS created_at,
                EXTRACT(EPOCH FROM w.updated_at)::bigint::text AS updated_at
         FROM cairn_project_worktrees w
         WHERE w.person_id = $1 AND {condition}
         ORDER BY w.path"
    )
}

fn project_branch_from_row(row: &PgRow) -> Result<ProjectBranch, PostgresError> {
    Ok(ProjectBranch {
        branch_id: row.try_get("branch_id")?,
        project_id: row.try_get("project_id")?,
        name: row.try_get("name")?,
        base_branch: row.try_get("base_branch")?,
        base_commit: row.try_get("base_commit")?,
        head_commit: row.try_get("head_commit")?,
        status: row.try_get("status")?,
        status_reason: row.try_get("status_reason")?,
        merge_commit: row.try_get("merge_commit")?,
        merge_memory_id: row.try_get("merge_memory_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn project_worktree_from_row(row: &PgRow) -> Result<ProjectWorktree, PostgresError> {
    Ok(ProjectWorktree {
        worktree_id: row.try_get("worktree_id")?,
        project_id: row.try_get("project_id")?,
        branch_id: row.try_get("branch_id")?,
        path: row.try_get("path")?,
        commit: row.try_get("commit_sha")?,
        status: row.try_get("status")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn project_entity_from_row(row: &PgRow) -> Result<ProjectEntity, PostgresError> {
    Ok(ProjectEntity {
        entity_id: row.try_get("entity_id")?,
        project_id: row.try_get("project_id")?,
        kind: row.try_get("kind")?,
        key: row.try_get("entity_key")?,
        content: row.try_get("content")?,
        source_memory_id: row.try_get("source_memory_id")?,
        commit: row.try_get("commit_sha")?,
        status: row.try_get("status")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn project_freshness_event_from_row(row: &PgRow) -> Result<ProjectFreshnessEvent, PostgresError> {
    Ok(ProjectFreshnessEvent {
        event_id: row.try_get("event_id")?,
        project_id: row.try_get("project_id")?,
        memory_id: row.try_get("memory_id")?,
        previous_commit: row.try_get("previous_commit")?,
        commit: row.try_get("commit_sha")?,
        action: row.try_get("action")?,
        reason: row.try_get("reason")?,
        created_at: row.try_get("created_at")?,
    })
}

fn device_select_columns() -> &'static str {
    "SELECT d.device_id, d.person_id, d.agent_id, d.name, d.status,
            EXTRACT(EPOCH FROM d.registered_at)::bigint::text AS registered_at,
            EXTRACT(EPOCH FROM d.last_seen_at)::bigint::text AS last_seen_at,
            d.pull_cursor AS cursor
     FROM cairn_devices d"
}

fn device_select_sql() -> String {
    format!(
        "SELECT x.*, COALESCE((SELECT a.status = 'active' FROM cairn_accounts a
                           WHERE a.person_id = x.person_id), false) AS account_active
         FROM ({} WHERE d.token_hash = $1) x",
        device_select_columns()
    )
}

fn device_from_row(row: &PgRow) -> Result<Device, PostgresError> {
    let status: String = row.try_get("status")?;
    let cursor: i64 = row.try_get("cursor")?;
    Ok(Device {
        device_id: row.try_get("device_id")?,
        person_id: row.try_get("person_id")?,
        agent_id: row.try_get("agent_id")?,
        name: row.try_get("name")?,
        status: match status.as_str() {
            "active" => DeviceStatus::Active,
            "revoked" => DeviceStatus::Revoked,
            other => {
                return Err(PostgresError::Validation(format!(
                    "unknown device status: {other}"
                )));
            }
        },
        registered_at: row.try_get("registered_at")?,
        last_seen_at: row.try_get("last_seen_at")?,
        cursor: cursor.max(0) as u64,
    })
}

fn sync_select_sql() -> &'static str {
    "SELECT se.cursor, se.event_id, se.person_id, se.device_id,
            o.event_id AS observation_event_id,
            o.session_id AS observation_session_id,
            o.content AS observation_content,
            o.role AS observation_role,
            o.source AS observation_source,
            o.importance AS observation_importance,
            o.lifecycle AS observation_lifecycle,
            o.scope AS observation_scope,
            o.context AS observation_context,
            EXTRACT(EPOCH FROM o.captured_at)::bigint::text AS observation_captured_at
     FROM cairn_sync_events se
     JOIN cairn_observations o
       ON o.person_id = se.person_id AND o.event_id = se.event_id
     WHERE se.person_id = $1"
}

fn sync_select_event_sql() -> String {
    format!("{} AND se.event_id = $2", sync_select_sql())
}

fn sync_event_from_row(row: &PgRow) -> Result<SyncEvent, PostgresError> {
    let cursor: i64 = row.try_get("cursor")?;
    Ok(SyncEvent {
        cursor: cursor.max(0) as u64,
        event_id: row.try_get("event_id")?,
        person_id: row.try_get("person_id")?,
        device_id: row.try_get("device_id")?,
        observation: observation_from_row(row)?,
    })
}

fn observation_from_row(row: &PgRow) -> Result<Observation, PostgresError> {
    let scope: String = row.try_get("observation_scope")?;
    Ok(Observation {
        event_id: row.try_get("observation_event_id")?,
        session_id: row.try_get("observation_session_id")?,
        content: row.try_get("observation_content")?,
        role: row.try_get("observation_role")?,
        source: row.try_get("observation_source")?,
        importance: row.try_get("observation_importance")?,
        lifecycle: row
            .try_get::<Json<Lifecycle>, _>("observation_lifecycle")?
            .0,
        scope: parse_scope(&scope)?,
        context: row
            .try_get::<Json<WorkContext>, _>("observation_context")?
            .0,
        captured_at: row.try_get("observation_captured_at")?,
    })
}

fn parse_scope(value: &str) -> Result<MemoryScope, PostgresError> {
    match value {
        "session" => Ok(MemoryScope::Session),
        "worktree" => Ok(MemoryScope::Worktree),
        "branch" => Ok(MemoryScope::Branch),
        "project" => Ok(MemoryScope::Project),
        "global" => Ok(MemoryScope::Global),
        other => Err(PostgresError::Validation(format!(
            "unknown memory scope: {other}"
        ))),
    }
}

fn parse_memory_kind(value: String) -> Result<MemoryKind, PostgresError> {
    match value.as_str() {
        "session_summary" => Ok(MemoryKind::SessionSummary),
        "explicit" => Ok(MemoryKind::Explicit),
        "lesson" => Ok(MemoryKind::Lesson),
        "correction" => Ok(MemoryKind::Correction),
        "decision" => Ok(MemoryKind::Decision),
        other => Err(PostgresError::Validation(format!(
            "unknown memory kind: {other}"
        ))),
    }
}

fn memory_kind_name(kind: &MemoryKind) -> &'static str {
    match kind {
        MemoryKind::SessionSummary => "session_summary",
        MemoryKind::Explicit => "explicit",
        MemoryKind::Lesson => "lesson",
        MemoryKind::Correction => "correction",
        MemoryKind::Decision => "decision",
    }
}

fn scope_name(scope: &MemoryScope) -> &'static str {
    match scope {
        MemoryScope::Session => "session",
        MemoryScope::Worktree => "worktree",
        MemoryScope::Branch => "branch",
        MemoryScope::Project => "project",
        MemoryScope::Global => "global",
    }
}

/// The cursor a pull page ends at. Events and deletions share one cursor sequence but are
/// capped separately. A list that hit the cap may hold more items past its last cursor, so
/// the page stops there; later items of the other list wait for the next pull. Both lists are
/// ordered by cursor, so the last cursor is the largest.
fn pull_page_end(
    last_event: Option<u64>,
    event_count: usize,
    last_deletion: Option<u64>,
    deletion_count: usize,
    floor: u64,
) -> u64 {
    let capped =
        |last: Option<u64>, count: usize| last.filter(|_| count >= MAX_PULL_EVENTS as usize);
    match (
        capped(last_event, event_count),
        capped(last_deletion, deletion_count),
    ) {
        (Some(event), Some(deletion)) => event.min(deletion),
        (Some(bound), None) | (None, Some(bound)) => bound,
        (None, None) => last_event.max(last_deletion).unwrap_or(floor),
    }
}

fn database_cursor(cursor: u64) -> Result<i64, PostgresError> {
    i64::try_from(cursor)
        .map_err(|_| PostgresError::Validation("cursor exceeds PostgreSQL range".to_owned()))
}

fn validate_project_text(value: &str, field: &str) -> Result<(), PostgresError> {
    if value.trim().is_empty() {
        return Err(PostgresError::Validation(format!(
            "{field} must not be empty"
        )));
    }
    if value.len() > 512 {
        return Err(PostgresError::Validation(format!(
            "{field} exceeds the 512 byte limit"
        )));
    }
    Ok(())
}

fn validate_entity_kind(kind: &str) -> Result<(), PostgresError> {
    match kind.trim() {
        "file" | "symbol" | "decision" | "handoff" | "runbook" => Ok(()),
        _ => Err(PostgresError::Validation(
            "entity kind must be file, symbol, decision, handoff, or runbook".to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_PULL_EVENTS, pull_page_end};

    #[test]
    fn pull_page_never_passes_an_undelivered_item() {
        let cap = MAX_PULL_EVENTS as usize;
        // Neither list capped: everything is delivered, so the page ends at the newest item.
        assert_eq!(pull_page_end(Some(9), 3, Some(12), 2, 4), 12);
        assert_eq!(pull_page_end(None, 0, None, 0, 4), 4);
        // Events capped at cursor 512 while the only deletion sits at 600: stop at 512.
        assert_eq!(pull_page_end(Some(512), cap, Some(600), 1, 0), 512);
        // Deletions capped at 512 while the newest event is 603: stop at 512.
        assert_eq!(pull_page_end(Some(603), 3, Some(512), cap, 0), 512);
        // Both capped: the earlier of the two limits.
        assert_eq!(pull_page_end(Some(1000), cap, Some(700), cap, 0), 700);
    }

    use super::session_memory_select_sql;

    #[test]
    fn session_memory_select_sql_filters_by_person_and_memory() {
        let sql = session_memory_select_sql();
        assert!(
            sql.contains("WHERE m.person_id = $1 AND m.memory_id = $2"),
            "session lookup must not bind parameters to a statement without placeholders: {sql}"
        );
        assert_eq!(sql.matches("$1").count(), 1);
        assert_eq!(sql.matches("$2").count(), 1);
    }
}
