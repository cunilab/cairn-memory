//! Password, email and token primitives (ported unchanged from 0.1.5).

use crate::error::Error;
use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng as PasswordRandomSource;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use rand::RngCore;
use rand::rngs::OsRng as TokenRandomSource;
use serde::Serialize;
use sha2::{Digest, Sha256};

pub const AUTH_SESSION_SECONDS: i64 = 30 * 24 * 60 * 60;
const MIN_PASSWORD_BYTES: usize = 12;

/// Servers map this error text to 403: a device token reached a web-only action.
pub const WEB_ONLY_ERROR: &str = "web console session required; device tokens cannot do this";
/// Servers map this error text to 403: a non-admin web session reached an admin action.
pub const ADMIN_ONLY_ERROR: &str = "admin web session required; only the admin account can do this";

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    Active,
    Disabled,
}

impl AccountStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Account {
    pub person_id: String,
    pub email: String,
    pub status: AccountStatus,
    /// Only the bootstrap account is an admin; it alone may create accounts.
    pub is_admin: bool,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub created_at: i64,
}

pub fn normalize_email(email: &str) -> Result<String, Error> {
    let normalized = email.trim().to_ascii_lowercase();
    let valid = normalized.len() <= 320
        && normalized.split('@').count() == 2
        && normalized
            .split('@')
            .all(|part| !part.is_empty() && !part.chars().any(char::is_whitespace));
    if !valid {
        return Err(Error::Validation(
            "email must be a valid account identifier".to_owned(),
        ));
    }
    Ok(normalized)
}

pub fn validate_password(password: &str) -> Result<(), Error> {
    let length = password.len();
    if !(MIN_PASSWORD_BYTES..=1024).contains(&length) {
        return Err(Error::Validation(format!(
            "password must be between {MIN_PASSWORD_BYTES} and 1024 bytes"
        )));
    }
    Ok(())
}

pub fn hash_password(password: &str) -> Result<String, Error> {
    let salt = SaltString::generate(&mut PasswordRandomSource);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| Error::Validation("password hashing failed".to_owned()))
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
mod tests {
    use super::*;

    #[test]
    fn email_and_password_rules() {
        assert_eq!(normalize_email(" A@B.io ").as_deref(), Ok("a@b.io"));
        assert!(normalize_email("no-at").is_err());
        assert!(validate_password("short").is_err());
        assert!(validate_password("long-enough-pw").is_ok());
    }

    #[test]
    fn hash_round_trip_and_tokens() {
        let hash = hash_password("long-enough-pw").expect("hash");
        assert!(verify_password("long-enough-pw", &hash));
        assert!(!verify_password("wrong-password!", &hash));
        let token = random_token("cairn-device");
        assert!(token.starts_with("cairn-device-"));
        assert_eq!(hash_token(&token).len(), 64);
    }
}
