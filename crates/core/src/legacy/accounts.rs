use crate::legacy::devices::{DeviceStatus, touch_last_seen};
use crate::legacy::store::MemoryStore;
use crate::legacy::types::{StoreError, now_seconds, timestamp, timestamp_nanos};
use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng as PasswordRandomSource;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use rand::RngCore;
use rand::rngs::OsRng as TokenRandomSource;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const AUTH_SESSION_SECONDS: u64 = 30 * 24 * 60 * 60;

/// Servers map this error text to 403: a device token reached a web-only action.
pub const WEB_ONLY_ERROR: &str = "web console session required; device tokens cannot do this";

/// Servers map this error text to 403: a non-admin web session reached an admin action.
pub const ADMIN_ONLY_ERROR: &str = "admin web session required; only the admin account can do this";

const MIN_PASSWORD_BYTES: usize = 12;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    #[default]
    Active,
    Disabled,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub person_id: String,
    pub email: String,
    #[serde(default)]
    pub status: AccountStatus,
    /// Only the bootstrap account is an admin; it alone may create accounts.
    #[serde(default)]
    pub is_admin: bool,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthSessionResult {
    pub account: Account,
    pub token: String,
    #[serde(rename = "expiresAt")]
    pub expires_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct AccountRecord {
    pub(crate) account: Account,
    password_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct AuthSessionRecord {
    session_id: String,
    person_id: String,
    token_hash: String,
    created_at: String,
    expires_at: String,
    revoked_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEvent {
    pub audit_id: String,
    pub action: String,
    pub subject_id: Option<String>,
    pub actor_device_id: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct StoredAuditEvent {
    pub(crate) person_id: String,
    pub(crate) audit_id: String,
    pub(crate) action: String,
    pub(crate) subject_id: Option<String>,
    pub(crate) actor_device_id: Option<String>,
    pub(crate) metadata: serde_json::Value,
    pub(crate) created_at: String,
}

impl MemoryStore {
    pub fn bootstrap_account(
        &mut self,
        email: &str,
        password: &str,
    ) -> Result<AuthSessionResult, StoreError> {
        if !self.state.accounts.is_empty() {
            return Err(StoreError::Validation(
                "account bootstrap has already completed".to_owned(),
            ));
        }
        let email = normalize_email(email)?;
        validate_password(password)?;
        let account = Account {
            person_id: format!("person-{}", timestamp_nanos()),
            email,
            status: AccountStatus::Active,
            is_admin: true,
            created_at: timestamp(),
        };
        self.state.accounts.push(AccountRecord {
            account: account.clone(),
            password_hash: hash_password(password)?,
        });
        let session = self.issue_auth_session(account)?;
        self.persist()?;
        Ok(session)
    }

    pub fn register_account(
        &mut self,
        email: &str,
        password: &str,
    ) -> Result<AuthSessionResult, StoreError> {
        if self.state.accounts.is_empty() {
            return Err(StoreError::Validation(
                "account bootstrap has not completed".to_owned(),
            ));
        }
        let email = normalize_email(email)?;
        validate_password(password)?;
        if self
            .state
            .accounts
            .iter()
            .any(|record| record.account.email == email)
        {
            return Err(StoreError::Validation(
                "account email is already registered".to_owned(),
            ));
        }
        let account = Account {
            person_id: format!("person-{}", timestamp_nanos()),
            email,
            status: AccountStatus::Active,
            is_admin: false,
            created_at: timestamp(),
        };
        self.state.accounts.push(AccountRecord {
            account: account.clone(),
            password_hash: hash_password(password)?,
        });
        let session = self.issue_auth_session(account)?;
        self.persist()?;
        Ok(session)
    }

    /// Admin web session only. A device token fails with `WEB_ONLY_ERROR`, a non-admin
    /// session with `ADMIN_ONLY_ERROR`.
    pub fn authenticate_admin(&self, token: &str) -> Result<Account, StoreError> {
        let account = self.authenticate_session(token)?;
        if account.is_admin {
            Ok(account)
        } else {
            Err(StoreError::Validation(ADMIN_ONLY_ERROR.to_owned()))
        }
    }

    pub fn register_account_as_admin(
        &mut self,
        token: &str,
        email: &str,
        password: &str,
    ) -> Result<AuthSessionResult, StoreError> {
        self.authenticate_admin(token)?;
        self.register_account(email, password)
    }

    pub fn login(&mut self, email: &str, password: &str) -> Result<AuthSessionResult, StoreError> {
        let email = normalize_email(email)?;
        let record = self
            .state
            .accounts
            .iter()
            .find(|record| record.account.email == email)
            .cloned()
            .ok_or_else(|| StoreError::Validation("invalid account credentials".to_owned()))?;
        if record.account.status != AccountStatus::Active
            || !verify_password(password, &record.password_hash)
        {
            return Err(StoreError::Validation(
                "invalid account credentials".to_owned(),
            ));
        }
        let session = self.issue_auth_session(record.account)?;
        self.persist()?;
        Ok(session)
    }

    /// Agent credential: a web session token or an active device token, resolved to the
    /// owning account.
    pub fn authenticate_account(&self, token: &str) -> Result<Account, StoreError> {
        self.authenticate_principal(token)
            .map(|(account, _)| account)
    }

    /// Web-only credential: a device token fails with `WEB_ONLY_ERROR`.
    pub fn authenticate_session(&self, token: &str) -> Result<Account, StoreError> {
        self.session_account(token).map_err(|error| {
            if self.is_device_token(token) {
                StoreError::Validation(WEB_ONLY_ERROR.to_owned())
            } else {
                error
            }
        })
    }

    /// True for any device token, including a revoked one.
    pub fn is_device_token(&self, token: &str) -> bool {
        let token_hash = hash_token(token);
        self.state
            .devices
            .iter()
            .any(|record| record.token_hash == token_hash)
    }

    /// Records device use in memory only, at most once per `LAST_SEEN_THROTTLE_SECS`; the next
    /// store write persists it.
    pub fn touch_device(&mut self, token: &str) {
        let token_hash = hash_token(token);
        if let Some(record) = self.state.devices.iter_mut().find(|record| {
            record.token_hash == token_hash && record.device.status == DeviceStatus::Active
        }) {
            touch_last_seen(&mut record.device);
        }
    }

    /// The account and whether the credential was a device token.
    pub(crate) fn authenticate_principal(
        &self,
        token: &str,
    ) -> Result<(Account, bool), StoreError> {
        let token_hash = hash_token(token);
        let Some(record) = self
            .state
            .devices
            .iter()
            .find(|record| record.token_hash == token_hash)
        else {
            return self.session_account(token).map(|account| (account, false));
        };
        if record.device.status != DeviceStatus::Active {
            return Err(StoreError::Validation("device is revoked".to_owned()));
        }
        self.state
            .accounts
            .iter()
            .find(|candidate| candidate.account.person_id == record.device.person_id)
            .map(|candidate| candidate.account.clone())
            .filter(|account| account.status == AccountStatus::Active)
            .map(|account| (account, true))
            .ok_or_else(|| StoreError::Validation("account authentication failed".to_owned()))
    }

    fn session_account(&self, token: &str) -> Result<Account, StoreError> {
        let token_hash = hash_token(token);
        let session = self
            .state
            .auth_sessions
            .iter()
            .find(|session| session.token_hash == token_hash)
            .ok_or_else(|| StoreError::Validation("account authentication failed".to_owned()))?;
        if session.revoked_at.is_some()
            || session
                .expires_at
                .parse::<u64>()
                .ok()
                .is_some_and(|expires_at| expires_at <= now_seconds())
        {
            return Err(StoreError::Validation(
                "account session is expired or revoked".to_owned(),
            ));
        }
        let account = self
            .state
            .accounts
            .iter()
            .find(|record| record.account.person_id == session.person_id)
            .map(|record| record.account.clone())
            .ok_or_else(|| StoreError::Validation("account authentication failed".to_owned()))?;
        if account.status != AccountStatus::Active {
            return Err(StoreError::Validation("account is disabled".to_owned()));
        }
        Ok(account)
    }

    pub fn logout_account(&mut self, token: &str) -> Result<(), StoreError> {
        if self.is_device_token(token) {
            return Err(StoreError::Validation(WEB_ONLY_ERROR.to_owned()));
        }
        let token_hash = hash_token(token);
        let session = self
            .state
            .auth_sessions
            .iter_mut()
            .find(|session| session.token_hash == token_hash)
            .ok_or_else(|| StoreError::Validation("account authentication failed".to_owned()))?;
        if session.revoked_at.is_none() {
            session.revoked_at = Some(timestamp());
            self.persist()?;
        }
        Ok(())
    }

    /// Admin web session only: every account, oldest first.
    pub fn accounts(&self, token: &str) -> Result<Vec<Account>, StoreError> {
        self.authenticate_admin(token)?;
        Ok(self
            .state
            .accounts
            .iter()
            .map(|record| record.account.clone())
            .collect())
    }

    /// Admin web session only. Sets another account's password and revokes every web
    /// session of that account; device tokens stay valid. Audited under the target.
    pub fn reset_account_password(
        &mut self,
        token: &str,
        person_id: &str,
        new_password: &str,
    ) -> Result<(), StoreError> {
        let admin = self.authenticate_admin(token)?;
        validate_password(new_password)?;
        let password_hash = hash_password(new_password)?;
        let record = self
            .state
            .accounts
            .iter_mut()
            .find(|record| record.account.person_id == person_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown account: {person_id}")))?;
        record.password_hash = password_hash;
        self.revoke_sessions_except(person_id, None);
        self.record_share_audit(
            person_id,
            "account_password_reset",
            Some(person_id),
            serde_json::json!({ "actor_person_id": admin.person_id }),
        );
        self.persist()
    }

    /// Web session only. Needs the current password, then revokes the account's other
    /// web sessions and keeps this one.
    pub fn change_password(
        &mut self,
        token: &str,
        current_password: &str,
        new_password: &str,
    ) -> Result<(), StoreError> {
        let account = self.authenticate_session(token)?;
        let record = self
            .state
            .accounts
            .iter_mut()
            .find(|record| record.account.person_id == account.person_id)
            .ok_or_else(|| StoreError::Validation("account authentication failed".to_owned()))?;
        if !verify_password(current_password, &record.password_hash) {
            return Err(StoreError::Validation(
                "invalid account credentials".to_owned(),
            ));
        }
        validate_password(new_password)?;
        record.password_hash = hash_password(new_password)?;
        self.revoke_sessions_except(&account.person_id, Some(&hash_token(token)));
        self.record_share_audit(
            &account.person_id,
            "account_password_change",
            Some(&account.person_id),
            serde_json::json!({}),
        );
        self.persist()
    }

    /// Stages the revocation of the person's open web sessions, except the one `keep`.
    fn revoke_sessions_except(&mut self, person_id: &str, keep: Option<&str>) {
        let revoked_at = timestamp();
        for session in &mut self.state.auth_sessions {
            if session.person_id == person_id
                && session.revoked_at.is_none()
                && keep != Some(session.token_hash.as_str())
            {
                session.revoked_at = Some(revoked_at.clone());
            }
        }
    }

    pub fn audit_events(
        &self,
        account_token: &str,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, StoreError> {
        let account = self.authenticate_session(account_token)?;
        Ok(self
            .state
            .audit_events
            .iter()
            .rev()
            .filter(|event| event.person_id == account.person_id)
            .take(limit)
            .map(|event| AuditEvent {
                audit_id: event.audit_id.clone(),
                action: event.action.clone(),
                subject_id: event.subject_id.clone(),
                actor_device_id: event.actor_device_id.clone(),
                metadata: event.metadata.clone(),
                created_at: event.created_at.clone(),
            })
            .collect())
    }

    fn issue_auth_session(&mut self, account: Account) -> Result<AuthSessionResult, StoreError> {
        let token = random_token("cairn-session");
        let token_hash = hash_token(&token);
        let expires_at = (now_seconds() + AUTH_SESSION_SECONDS).to_string();
        let session_id = format!("auth-{}", &token_hash[..16]);
        self.state.auth_sessions.push(AuthSessionRecord {
            session_id,
            person_id: account.person_id.clone(),
            token_hash,
            created_at: timestamp(),
            expires_at: expires_at.clone(),
            revoked_at: None,
        });
        Ok(AuthSessionResult {
            account,
            token,
            expires_at,
        })
    }

    pub fn has_accounts(&self) -> bool {
        !self.state.accounts.is_empty()
    }
}

pub fn normalize_email(email: &str) -> Result<String, StoreError> {
    let normalized = email.trim().to_ascii_lowercase();
    let valid = normalized.len() <= 320
        && normalized.split('@').count() == 2
        && normalized
            .split('@')
            .all(|part| !part.is_empty() && !part.chars().any(char::is_whitespace));
    if !valid {
        return Err(StoreError::Validation(
            "email must be a valid account identifier".to_owned(),
        ));
    }
    Ok(normalized)
}

pub fn validate_password(password: &str) -> Result<(), StoreError> {
    let length = password.len();
    if !(MIN_PASSWORD_BYTES..=1024).contains(&length) {
        return Err(StoreError::Validation(format!(
            "password must be between {MIN_PASSWORD_BYTES} and 1024 bytes"
        )));
    }
    Ok(())
}

pub fn hash_password(password: &str) -> Result<String, StoreError> {
    let salt = SaltString::generate(&mut PasswordRandomSource);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| StoreError::Validation("password hashing failed".to_owned()))
}

pub fn verify_password(password: &str, encoded_hash: &str) -> bool {
    let Ok(hash) = PasswordHash::new(encoded_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &hash)
        .is_ok()
}

pub fn random_token(prefix: &str) -> String {
    let mut bytes = [0_u8; 32];
    TokenRandomSource.fill_bytes(&mut bytes);
    let encoded = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{prefix}-{encoded}")
}

pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests;
