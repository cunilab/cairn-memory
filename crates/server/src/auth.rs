//! Who is calling. A web session and a device token both resolve to an account; some actions
//! take only the web session. Extractors keep the check in each handler's signature.

use crate::AppState;
use crate::error::ApiError;
use axum::extract::FromRequestParts;
use axum::http::HeaderMap;
use axum::http::request::Parts;
use cairn_core::account::{ADMIN_ONLY_ERROR, Account, DEVICE_TOKEN_PREFIX, WEB_ONLY_ERROR};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const ATTEMPT_LIMIT: u32 = 10;
const ATTEMPT_WINDOW: Duration = Duration::from_secs(60);
const PRUNE_AT: usize = 1024;

#[derive(Default)]
pub struct AuthRateLimiter {
    attempts: Mutex<HashMap<String, (Instant, u32)>>,
}

impl AuthRateLimiter {
    /// Ten tries per key per minute. Keys come from callers who are not signed in, so expired
    /// windows are dropped to keep the map bounded.
    pub fn allow(&self, operation: &str, email: &str) -> bool {
        let key = format!("{operation}:{}", email.trim().to_ascii_lowercase());
        let now = Instant::now();
        let mut attempts = self
            .attempts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if attempts.len() >= PRUNE_AT {
            attempts.retain(|_, (started, _)| now.duration_since(*started) < ATTEMPT_WINDOW);
        }
        let entry = attempts.entry(key).or_insert((now, 0));
        if now.duration_since(entry.0) >= ATTEMPT_WINDOW {
            *entry = (now, 0);
        }
        if entry.1 >= ATTEMPT_LIMIT {
            return false;
        }
        entry.1 += 1;
        true
    }
}

pub fn optional_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.trim().is_empty())
}

pub fn token(headers: &HeaderMap) -> Result<&str, ApiError> {
    optional_token(headers).ok_or_else(|| ApiError::unauthorized("authentication failed"))
}

impl AppState {
    /// A web session, or an active device token, resolved to its account.
    pub async fn agent(&self, headers: &HeaderMap) -> Result<Account, ApiError> {
        let token = token(headers)?;
        if let Ok(account) = self.store.session_account(token).await {
            return Ok(account);
        }
        match self.store.authenticate_device(token).await {
            Ok((_, account)) => Ok(account),
            Err(_) => Err(ApiError::unauthorized("authentication failed")),
        }
    }

    /// A web session only. A device token reaches 403, any other bearer 401.
    pub async fn web(&self, headers: &HeaderMap) -> Result<Account, ApiError> {
        let token = token(headers)?;
        match self.store.session_account(token).await {
            Ok(account) => Ok(account),
            Err(_) if token.starts_with(DEVICE_TOKEN_PREFIX) => {
                Err(ApiError::forbidden(WEB_ONLY_ERROR))
            }
            Err(_) => Err(ApiError::unauthorized("authentication failed")),
        }
    }

    pub async fn admin(&self, headers: &HeaderMap) -> Result<Account, ApiError> {
        let account = self.web(headers).await?;
        if account.is_admin {
            Ok(account)
        } else {
            Err(ApiError::forbidden(ADMIN_ONLY_ERROR))
        }
    }

    /// Login and bootstrap are for people at the console, not for a device token.
    pub fn reject_device_bearer(&self, headers: &HeaderMap) -> Result<(), ApiError> {
        match optional_token(headers) {
            Some(t) if t.starts_with(DEVICE_TOKEN_PREFIX) => {
                Err(ApiError::forbidden(WEB_ONLY_ERROR))
            }
            _ => Ok(()),
        }
    }

    pub fn throttle(&self, operation: &str, email: &str) -> Result<(), ApiError> {
        if self.limiter.allow(operation, email) {
            Ok(())
        } else {
            Err(ApiError::rate_limited())
        }
    }
}

/// Handler argument: the caller as an account, by web session or device token.
pub struct Agent(pub Account);
/// Handler argument: the caller as an account, by web session only.
pub struct Web(pub Account);
/// Handler argument: the admin account, by web session only.
pub struct Admin(pub Account);

macro_rules! extractor {
    ($name:ident, $method:ident) => {
        impl FromRequestParts<AppState> for $name {
            type Rejection = ApiError;
            async fn from_request_parts(
                parts: &mut Parts,
                state: &AppState,
            ) -> Result<Self, Self::Rejection> {
                state.$method(&parts.headers).await.map($name)
            }
        }
    };
}
extractor!(Agent, agent);
extractor!(Web, web);
extractor!(Admin, admin);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_stops_the_eleventh_try() {
        let limiter = AuthRateLimiter::default();
        for _ in 0..10 {
            assert!(limiter.allow("login", "A@x.io"));
        }
        assert!(!limiter.allow("login", " a@x.io "));
        assert!(limiter.allow("login", "b@x.io"));
    }
}
