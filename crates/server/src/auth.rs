//! Who is calling. A web session and a device token both resolve to an account; some actions
//! take only the web session. Extractors keep the check in each handler's signature.
//!
//! Error codes are the contract with the web: `session_required` means "sign in again" (no
//! session, an unknown or expired or revoked one, or a device token on a console route);
//! `unauthorized` is a device token that is revoked or not valid.

use crate::AppState;
use crate::error::ApiError;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, header::USER_AGENT};
use cairn_core::account::{ADMIN_ONLY_ERROR, Account, DEVICE_TOKEN_PREFIX, parse_user_agent};

/// The text sent with `session_required`; the code, not the text, is what clients switch on.
const EXPIRED_SESSION: &str = "account session is expired or revoked";
const BEARER_REQUIRED: &str = "Bearer token is required";

pub fn optional_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.trim().is_empty())
}

/// For a device-token route.
pub fn token(headers: &HeaderMap) -> Result<&str, ApiError> {
    optional_token(headers).ok_or_else(|| ApiError::unauthorized(BEARER_REQUIRED))
}

/// For a route the console calls: no bearer means no session.
pub fn web_token(headers: &HeaderMap) -> Result<&str, ApiError> {
    optional_token(headers).ok_or_else(|| ApiError::session_required(BEARER_REQUIRED))
}

/// An account reached by web session or device token; `device_id` is set for a device.
pub struct Caller {
    pub account: Account,
    pub device_id: Option<String>,
}

/// An account reached by an active device token.
pub struct DeviceCaller {
    pub account: Account,
    pub device_id: String,
}

impl AppState {
    /// A web session, or an active device token, resolved to its account.
    pub async fn caller(&self, headers: &HeaderMap) -> Result<Caller, ApiError> {
        let token = web_token(headers)?;
        if let Ok(account) = self.store.session_account(token).await {
            return Ok(Caller {
                account,
                device_id: None,
            });
        }
        match self.store.authenticate_device(token).await {
            Ok((device, account)) => {
                self.note_client(headers, &device.device_id).await;
                Ok(Caller {
                    account,
                    device_id: Some(device.device_id),
                })
            }
            Err(_) if token.starts_with(DEVICE_TOKEN_PREFIX) => {
                Err(ApiError::unauthorized("device is revoked"))
            }
            Err(_) => Err(ApiError::session_required(EXPIRED_SESSION)),
        }
    }

    pub async fn agent(&self, headers: &HeaderMap) -> Result<Account, ApiError> {
        Ok(self.caller(headers).await?.account)
    }

    /// An active device token only: captures come from agents, never from a console session.
    pub async fn device_caller(&self, headers: &HeaderMap) -> Result<DeviceCaller, ApiError> {
        let token = token(headers)?;
        match self.store.authenticate_device(token).await {
            Ok((device, account)) => {
                self.note_client(headers, &device.device_id).await;
                Ok(DeviceCaller {
                    account,
                    device_id: device.device_id,
                })
            }
            Err(_) if token.starts_with(DEVICE_TOKEN_PREFIX) => {
                Err(ApiError::unauthorized("device is revoked"))
            }
            Err(_) => Err(ApiError::unauthorized("authentication failed")),
        }
    }

    pub async fn device(&self, headers: &HeaderMap) -> Result<Account, ApiError> {
        Ok(self.device_caller(headers).await?.account)
    }

    /// A web session only. A device token is not one, so it is sent to sign in like any other
    /// bearer that is not a session.
    pub async fn web(&self, headers: &HeaderMap) -> Result<Account, ApiError> {
        self.store
            .session_account(web_token(headers)?)
            .await
            .map_err(|_| ApiError::session_required(EXPIRED_SESSION))
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
                Err(ApiError::forbidden(cairn_core::account::WEB_ONLY_ERROR))
            }
            _ => Ok(()),
        }
    }

    /// Remembers which client a device is (`User-Agent: cairn/<version> (<agent>; <kind>)`), for
    /// the console's device health. Never fails a request.
    async fn note_client(&self, headers: &HeaderMap, device_id: &str) {
        let Some(info) = headers
            .get(USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_user_agent)
        else {
            return;
        };
        if let Err(error) = self.store.note_client(device_id, &info).await {
            eprintln!("could not record the device client: {error}");
        }
    }
}

/// Handler argument: the caller as an account, by web session or device token.
pub struct Agent(pub Account);
/// Handler argument: the caller as an account, by device token only.
pub struct Device(pub Account);
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
extractor!(Device, device);
extractor!(Web, web);
extractor!(Admin, admin);

impl FromRequestParts<AppState> for Caller {
    type Rejection = ApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        state.caller(&parts.headers).await
    }
}

impl FromRequestParts<AppState> for DeviceCaller {
    type Rejection = ApiError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        state.device_caller(&parts.headers).await
    }
}
