use crate::{Result, Store, StoreError, epoch};
use cairn_core::account::{
    AUTH_SESSION_SECONDS, Account, AccountStatus, AuditEvent, hash_password, hash_token,
    normalize_email, random_token, validate_password, verify_password,
};
use cairn_core::id::{new_id, now_seconds};
use sqlx::Row;
use sqlx::postgres::PgRow;

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

    pub async fn login(&self, email: &str, password: &str) -> Result<AuthSession> {
        let invalid = || StoreError::Forbidden("invalid email or password".into());
        let email = normalize_email(email).map_err(|_| invalid())?;
        let row = sqlx::query(&format!(
            "SELECT {ACCOUNT_COLUMNS}, password_hash FROM accounts WHERE email = $1"
        ))
        .bind(&email)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Err(invalid());
        };
        let account = account_from_row(&row)?;
        let stored: String = row.try_get("password_hash")?;
        if !verify_password(password, &stored) || account.status != AccountStatus::Active {
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
        if !stored.is_some_and(|hash| verify_password(current, &hash)) {
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
            "SELECT {ACCOUNT_COLUMNS} FROM accounts ORDER BY created_at, person_id"
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
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Newest first.
    pub async fn list_audit(&self, person_id: &str, limit: i64) -> Result<Vec<AuditEvent>> {
        let rows = sqlx::query(
            "SELECT audit_id, device_id, action, subject_id, metadata,
                    EXTRACT(EPOCH FROM created_at)::bigint AS created_at
             FROM audit_events WHERE person_id = $1 ORDER BY created_at DESC, audit_id LIMIT $2",
        )
        .bind(person_id)
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(AuditEvent {
                    audit_id: row.try_get("audit_id")?,
                    action: row.try_get("action")?,
                    subject_id: row.try_get("subject_id")?,
                    device_id: row.try_get("device_id")?,
                    metadata: row.try_get("metadata")?,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect()
    }
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
