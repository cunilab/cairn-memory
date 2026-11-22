use crate::{Result, Store, StoreError, epoch};
use cairn_core::account::{
    AUTH_SESSION_SECONDS, Account, AccountStatus, AuditEvent, SYSTEM_ACTIONS, hash_password,
    hash_token, normalize_email, random_token, validate_password, verify_password,
};
use cairn_core::id::{new_id, now_seconds};
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};
use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

/// Verified against when there is no account, so every login does one Argon2 verify.
static DUMMY_HASH: OnceLock<String> = OnceLock::new();

/// One Argon2 verify on every path; true only for a stored hash that matches.
fn check_password(stored: Option<&str>, given: &str) -> bool {
    let hash = stored.unwrap_or_else(|| {
        DUMMY_HASH.get_or_init(|| hash_password("cairn-timing-dummy").expect("argon2"))
    });
    verify_password(given, hash) && stored.is_some()
}

/// Which side of the audit log a reader wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuditActor {
    /// What Cairn did on its own.
    System,
    /// What a person or their devices did.
    Person,
}

#[derive(Clone, Debug, Default)]
pub struct AuditQuery {
    /// 1..=200, default 50.
    pub limit: Option<i64>,
    /// Epoch seconds; with `before_id`, the row it names is the last one already seen.
    pub before: Option<i64>,
    pub before_id: Option<String>,
    pub actor: Option<AuditActor>,
    /// Exact action names; empty means all.
    pub types: Vec<String>,
}

/// A web console login: the bearer token is shown once and stored hashed.
#[derive(Clone, Debug)]
pub struct AuthSession {
    pub account: Account,
    pub token: String,
    pub expires_at: i64,
}

const ACCOUNT_COLUMNS: &str =
    "person_id, email, is_admin, status, EXTRACT(EPOCH FROM created_at)::bigint AS created_at";

pub(crate) fn account_from_row(row: &PgRow) -> Result<Account> {
    Ok(Account {
        person_id: row.try_get("person_id")?,
        email: row.try_get("email")?,
        is_admin: row.try_get("is_admin")?,
        status: if row.try_get::<String, _>("status")? == "disabled" {
            AccountStatus::Disabled
        } else {
            AccountStatus::Active
        },
        created_at: row.try_get("created_at")?,
    })
}

impl Store {
    /// The first account becomes the admin. A second bootstrap is refused.
    pub async fn bootstrap(&self, email: &str, password: &str) -> Result<AuthSession> {
        let email = normalize_email(email)?;
        validate_password(password)?;
        let hash = hash_password(password)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("LOCK TABLE accounts IN EXCLUSIVE MODE")
            .execute(&mut *tx)
            .await?;
        if sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM accounts)")
            .fetch_one(&mut *tx)
            .await?
        {
            return Err(StoreError::Validation(
                "account bootstrap has already completed".into(),
            ));
        }
        let account = insert_account(&mut tx, &email, &hash, true).await?;
        let session = issue_session(&mut tx, account).await?;
        tx.commit().await?;
        Ok(session)
    }

    /// False until the first account exists; the web console then offers first-account setup.
    pub async fn has_accounts(&self) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accounts)")
            .fetch_one(&self.pool)
            .await?)
    }

    /// A normal account. The caller (the server) decides that only an admin may call this.
    pub async fn register(&self, email: &str, password: &str) -> Result<Account> {
        let email = normalize_email(email)?;
        validate_password(password)?;
        let hash = hash_password(password)?;
        let mut tx = self.pool.begin().await?;
        if !sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM accounts)")
            .fetch_one(&mut *tx)
            .await?
        {
            return Err(StoreError::Validation(
                "account bootstrap has not completed".into(),
            ));
        }
        let account = insert_account(&mut tx, &email, &hash, false).await?;
        tx.commit().await?;
        Ok(account)
    }

    /// The same work for an unknown email, a disabled account and a wrong password: one Argon2
    /// verify, then one error, so neither the answer nor its time says which it was.
    pub async fn login(&self, email: &str, password: &str) -> Result<AuthSession> {
        let invalid = || StoreError::Forbidden("invalid email or password".into());
        let Ok(email) = normalize_email(email) else {
            check_password(None, password);
            return Err(invalid());
        };
        let row = sqlx::query(&format!(
            "SELECT {ACCOUNT_COLUMNS}, password_hash FROM accounts WHERE email = $1"
        ))
        .bind(&email)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            check_password(None, password);
            return Err(invalid());
        };
        let account = account_from_row(&row)?;
        let stored: String = row.try_get("password_hash")?;
        if !check_password(Some(&stored), password) || account.status != AccountStatus::Active {
            return Err(invalid());
        }
        let mut tx = self.pool.begin().await?;
        let session = issue_session(&mut tx, account).await?;
        tx.commit().await?;
        Ok(session)
    }

    /// The account behind an unexpired, unrevoked web token.
    pub async fn session_account(&self, token: &str) -> Result<Account> {
        let row = sqlx::query(&format!(
            "SELECT a.person_id, a.email, a.is_admin, a.status, {} AS created_at
             FROM web_sessions s JOIN accounts a USING (person_id)
             WHERE s.token_hash = $1 AND s.revoked_at IS NULL AND s.expires_at > now()
               AND a.status = 'active'",
            epoch!("a.created_at")
        ))
        .bind(hash_token(token))
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => account_from_row(&row),
            None => Err(StoreError::Forbidden("invalid or expired session".into())),
        }
    }

    pub async fn logout(&self, token: &str) -> Result<()> {
        sqlx::query("UPDATE web_sessions SET revoked_at = now() WHERE token_hash = $1 AND revoked_at IS NULL")
            .bind(hash_token(token))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Own password change. All web sessions are revoked, and the caller logs in again.
    pub async fn change_password(&self, person_id: &str, current: &str, new: &str) -> Result<()> {
        validate_password(new)?;
        let stored: Option<String> =
            sqlx::query_scalar("SELECT password_hash FROM accounts WHERE person_id = $1")
                .bind(person_id)
                .fetch_optional(&self.pool)
                .await?;
        if !check_password(stored.as_deref(), current) {
            return Err(StoreError::Forbidden("current password is wrong".into()));
        }
        self.set_password(person_id, new).await
    }

    /// Admin reset of another account's password.
    pub async fn admin_set_password(&self, person_id: &str, new: &str) -> Result<()> {
        validate_password(new)?;
        self.set_password(person_id, new).await
    }

    async fn set_password(&self, person_id: &str, new: &str) -> Result<()> {
        let hash = hash_password(new)?;
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query("UPDATE accounts SET password_hash = $2 WHERE person_id = $1")
            .bind(person_id)
            .bind(hash)
            .execute(&mut *tx)
            .await?;
        if updated.rows_affected() == 0 {
            return Err(StoreError::NotFound("account".into()));
        }
        sqlx::query("UPDATE web_sessions SET revoked_at = now() WHERE person_id = $1 AND revoked_at IS NULL")
            .bind(person_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn list_accounts(&self) -> Result<Vec<Account>> {
        let rows = sqlx::query(&format!(
            "SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE person_id <> '{}' ORDER BY created_at, person_id",
            crate::consolidate::SYSTEM_OWNER
        ))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(account_from_row).collect()
    }

    pub async fn record_audit(
        &self,
        person_id: &str,
        device_id: Option<&str>,
        action: &str,
        subject_id: Option<&str>,
        metadata: serde_json::Value,
    ) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        audit_tx(
            &mut conn, person_id, device_id, action, subject_id, metadata,
        )
        .await
    }

    /// Newest first, `limit` rows. The next page is the rows before the last one returned
    /// (`before` and `before_id`). Titles are resolved for the memories the person may still see.
    pub async fn list_audit(&self, person_id: &str, query: &AuditQuery) -> Result<Vec<AuditEvent>> {
        let actor = match query.actor {
            None => "any",
            Some(AuditActor::System) => "system",
            Some(AuditActor::Person) => "person",
        };
        let system: Vec<&str> = SYSTEM_ACTIONS.to_vec();
        let rows = sqlx::query(&format!(
            "SELECT a.audit_id, a.device_id, a.action, a.subject_id, a.metadata, {} AS created_at, d.name AS device_name
               FROM audit_events a
               LEFT JOIN devices d ON d.person_id = a.person_id
                                  AND d.device_id = COALESCE(a.device_id, a.metadata ->> 'device_id')
              WHERE a.person_id = $1
                AND (cardinality($2::text[]) = 0 OR a.action = ANY($2))
                AND ($3 = 'any' OR ($3 = 'system') = (a.action = ANY($4)))
                AND ($5::bigint IS NULL OR {at} < $5
                     OR ($6::text IS NOT NULL AND {at} = $5 AND a.audit_id < $6))
              ORDER BY {at} DESC, a.audit_id DESC LIMIT $7",
            epoch!("a.created_at"),
            at = epoch!("a.created_at"),
        ))
        .bind(person_id)
        .bind(&query.types)
        .bind(actor)
        .bind(&system)
        .bind(query.before)
        .bind(&query.before_id)
        .bind(query.limit.unwrap_or(50).clamp(1, 200))
        .fetch_all(&self.pool)
        .await?;
        let mut events = rows
            .iter()
            .map(|row| {
                Ok(AuditEvent {
                    audit_id: row.try_get("audit_id")?,
                    action: row.try_get("action")?,
                    subject_id: row.try_get("subject_id")?,
                    device_id: row.try_get("device_id")?,
                    device_name: row.try_get("device_name")?,
                    metadata: row.try_get("metadata")?,
                    titles: BTreeMap::new(),
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        self.resolve_titles(person_id, &mut events).await?;
        Ok(events)
    }

    /// Fills `titles` from the memory ids in each event's subject and metadata. A memory the
    /// person cannot see (another person's, forgotten) is left out, so an event never leaks one.
    async fn resolve_titles(&self, person_id: &str, events: &mut [AuditEvent]) -> Result<()> {
        let mut named: Vec<Vec<String>> = Vec::with_capacity(events.len());
        let mut all = HashSet::new();
        for event in events.iter() {
            let mut ids = Vec::new();
            if let Some(subject) = &event.subject_id {
                collect_memory_ids(&serde_json::Value::String(subject.clone()), &mut ids);
            }
            collect_memory_ids(&event.metadata, &mut ids);
            all.extend(ids.iter().cloned());
            named.push(ids);
        }
        if all.is_empty() {
            return Ok(());
        }
        let all: Vec<String> = all.into_iter().collect();
        let titles: BTreeMap<String, String> = sqlx::query(&format!(
            "SELECT m.memory_id, m.title FROM memories m WHERE m.memory_id = ANY($2) AND {}",
            crate::memory::visible_even_if_expired("m", "$1")
        ))
        .bind(person_id)
        .bind(&all)
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(|r| Ok((r.try_get("memory_id")?, r.try_get("title")?)))
        .collect::<Result<_>>()?;
        for (event, ids) in events.iter_mut().zip(named) {
            event.titles = ids
                .into_iter()
                .filter_map(|id| titles.get(&id).map(|t| (id, t.clone())))
                .collect();
        }
        Ok(())
    }
}

/// Every string in `value` that is a memory id (`mem_...`, or `hnd_...` for a handoff).
fn collect_memory_ids(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text)
            if (text.starts_with("mem_") || text.starts_with("hnd_")) && text.len() <= 64 =>
        {
            out.push(text.clone());
        }
        serde_json::Value::Array(items) => items.iter().for_each(|v| collect_memory_ids(v, out)),
        serde_json::Value::Object(map) => map.values().for_each(|v| collect_memory_ids(v, out)),
        _ => {}
    }
}

/// `record_audit` inside a transaction or on a held connection.
pub(crate) async fn audit_tx(
    conn: &mut PgConnection,
    person_id: &str,
    device_id: Option<&str>,
    action: &str,
    subject_id: Option<&str>,
    metadata: serde_json::Value,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO audit_events (audit_id, person_id, device_id, action, subject_id, metadata)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(new_id("aud"))
    .bind(person_id)
    .bind(device_id)
    .bind(action)
    .bind(subject_id)
    .bind(metadata)
    .execute(conn)
    .await?;
    Ok(())
}

async fn insert_account(
    tx: &mut sqlx::PgConnection,
    email: &str,
    password_hash: &str,
    is_admin: bool,
) -> Result<Account> {
    let row = sqlx::query(&format!(
        "INSERT INTO accounts (person_id, email, password_hash, is_admin) VALUES ($1, $2, $3, $4)
         ON CONFLICT (email) DO NOTHING RETURNING {ACCOUNT_COLUMNS}"
    ))
    .bind(new_id("person"))
    .bind(email)
    .bind(password_hash)
    .bind(is_admin)
    .fetch_optional(tx)
    .await?;
    match row {
        Some(row) => account_from_row(&row),
        None => Err(StoreError::Validation(
            "account email is already registered".into(),
        )),
    }
}

async fn issue_session(tx: &mut sqlx::PgConnection, account: Account) -> Result<AuthSession> {
    let token = random_token("cairn-web");
    let expires_at = now_seconds() + AUTH_SESSION_SECONDS;
    sqlx::query(
        "INSERT INTO web_sessions (session_id, person_id, token_hash, expires_at)
         VALUES ($1, $2, $3, to_timestamp($4))",
    )
    .bind(new_id("ws"))
    .bind(&account.person_id)
    .bind(hash_token(&token))
    .bind(expires_at as f64)
    .execute(tx)
    .await?;
    Ok(AuthSession {
        account,
        token,
        expires_at,
    })
}
