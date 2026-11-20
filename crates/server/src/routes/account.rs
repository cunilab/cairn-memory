use crate::AppState;
use crate::auth::{Admin, Agent, Web, token};
use crate::error::{ApiError, ApiResult};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use cairn_core::account::{Account, AuditEvent};
use cairn_store::StoreError;
use cairn_store::account::AuthSession;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct Credentials {
    email: String,
    password: String,
}

#[derive(Deserialize)]
pub struct PasswordReset {
    password: String,
}

#[derive(Deserialize)]
pub struct PasswordChange {
    current_password: String,
    new_password: String,
}

#[derive(Serialize)]
pub struct Session {
    account: Account,
    token: String,
    expires_at: String,
}

impl From<AuthSession> for Session {
    fn from(session: AuthSession) -> Self {
        Self {
            account: session.account,
            token: session.token,
            expires_at: session.expires_at.to_string(),
        }
    }
}

pub async fn bootstrap(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Credentials>,
) -> ApiResult<Session> {
    state.reject_device_bearer(&headers)?;
    state.throttle("bootstrap", &body.email)?;
    Ok(Json(
        state
            .store
            .bootstrap(&body.email, &body.password)
            .await?
            .into(),
    ))
}

pub async fn register(
    State(state): State<AppState>,
    Admin(_): Admin,
    Json(body): Json<Credentials>,
) -> ApiResult<Account> {
    // The admin check comes first, so a caller without it cannot use up an email's quota.
    state.throttle("register", &body.email)?;
    Ok(Json(
        state.store.register(&body.email, &body.password).await?,
    ))
}

pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Credentials>,
) -> ApiResult<Session> {
    state.reject_device_bearer(&headers)?;
    state.throttle("login", &body.email)?;
    match state.store.login(&body.email, &body.password).await {
        Ok(session) => Ok(Json(session.into())),
        // The console shows this exact text for a failed sign-in.
        Err(StoreError::Forbidden(_)) => Err(ApiError::unauthorized("invalid account credentials")),
        Err(error) => Err(error.into()),
    }
}

pub async fn logout(
    State(state): State<AppState>,
    Web(_): Web,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    state.store.logout(token(&headers)?).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn me(Agent(account): Agent) -> Json<Account> {
    Json(account)
}

pub async fn audit(State(state): State<AppState>, Web(account): Web) -> ApiResult<Vec<AuditEvent>> {
    Ok(Json(state.store.list_audit(&account.person_id, 100).await?))
}

pub async fn accounts(State(state): State<AppState>, Admin(_): Admin) -> ApiResult<Vec<Account>> {
    Ok(Json(state.store.list_accounts().await?))
}

pub async fn reset_password(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(person_id): Path<String>,
    Json(body): Json<PasswordReset>,
) -> Result<StatusCode, ApiError> {
    state
        .store
        .admin_set_password(&person_id, &body.password)
        .await?;
    state
        .store
        .record_audit(
            &admin.person_id,
            None,
            "password_reset",
            Some(&person_id),
            serde_json::json!({}),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn change_password(
    State(state): State<AppState>,
    Web(account): Web,
    Json(body): Json<PasswordChange>,
) -> Result<StatusCode, ApiError> {
    // Authentication came first, so a caller without a session cannot use up an email's quota.
    state.throttle("password", &account.email)?;
    match state
        .store
        .change_password(
            &account.person_id,
            &body.current_password,
            &body.new_password,
        )
        .await
    {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        // The password dialog shows this exact text for a wrong current password.
        Err(StoreError::Forbidden(_)) => Err(ApiError::unauthorized("invalid account credentials")),
        Err(error) => Err(error.into()),
    }
}
