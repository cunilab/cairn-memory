use crate::AppState;
use crate::auth::{Admin, Agent, Web, token};
use crate::error::{ApiError, ApiResult};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use cairn_core::account::{Account, AuditEvent};
use cairn_store::StoreError;
use cairn_store::account::AuthSession;
use cairn_store::account::{AuditActor, AuditQuery};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Serialize)]
pub struct Session {
    account: Account,
    token: String,
    /// Epoch seconds; the console's cookie lives until then.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    expires_at: i64,
}

impl From<AuthSession> for Session {
    fn from(session: AuthSession) -> Self {
        Self {
            account: session.account,
            token: session.token,
            expires_at: session.expires_at,
        }
    }
}

#[derive(Deserialize)]
pub struct AuditParams {
    limit: Option<i64>,
    before: Option<i64>,
    before_id: Option<String>,
    /// `system` (what Cairn did) or `person` (what people and their devices did).
    actor: Option<String>,
    /// Action names, comma separated.
    #[serde(rename = "type")]
    kinds: Option<String>,
}

pub async fn bootstrap(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Credentials>,
) -> ApiResult<Session> {
    state.reject_device_bearer(&headers)?;
    let session = state.store.bootstrap(&body.email, &body.password).await?;
    let person = session.account.person_id.clone();
    for action in ["account_registered", "login"] {
        state
            .store
            .record_audit(
                &person,
                None,
                action,
                Some(&person),
                serde_json::json!({"first_admin": true}),
            )
            .await?;
    }
    Ok(Json(session.into()))
}

pub async fn register(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Json(body): Json<Credentials>,
) -> ApiResult<Account> {
    let account = state.store.register(&body.email, &body.password).await?;
    state
        .store
        .record_audit(
            &admin.person_id,
            None,
            "account_registered",
            Some(&account.person_id),
            serde_json::json!({"email": account.email}),
        )
        .await?;
    Ok(Json(account))
}

pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Credentials>,
) -> ApiResult<Session> {
    state.reject_device_bearer(&headers)?;
    match state.store.login(&body.email, &body.password).await {
        Ok(session) => {
            // Failed sign-ins are not recorded: there is no per-email state, by design.
            state
                .store
                .record_audit(
                    &session.account.person_id,
                    None,
                    "login",
                    None,
                    serde_json::json!({}),
                )
                .await?;
            Ok(Json(session.into()))
        }
        Err(StoreError::Forbidden(_)) => Err(ApiError::invalid_credentials()),
        Err(error) => Err(error.into()),
    }
}

pub async fn logout(
    State(state): State<AppState>,
    Web(account): Web,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    state.store.logout(token(&headers)?).await?;
    state
        .store
        .record_audit(
            &account.person_id,
            None,
            "logout",
            None,
            serde_json::json!({}),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn me(Agent(account): Agent) -> Json<Account> {
    Json(account)
}

/// Newest first. The next page: `before` and `before_id` from the last event returned.
pub async fn audit(
    State(state): State<AppState>,
    Web(account): Web,
    Query(params): Query<AuditParams>,
) -> ApiResult<Vec<AuditEvent>> {
    let actor = match params.actor.as_deref() {
        None | Some("") => None,
        Some("system") => Some(AuditActor::System),
        Some("person") => Some(AuditActor::Person),
        Some(_) => return Err(ApiError::invalid("actor must be system or person")),
    };
    let query = AuditQuery {
        limit: params.limit,
        before: params.before,
        before_id: params.before_id,
        actor,
        types: params
            .kinds
            .iter()
            .flat_map(|k| k.split(','))
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(str::to_owned)
            .collect(),
    };
    Ok(Json(
        state.store.list_audit(&account.person_id, &query).await?,
    ))
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
    match state
        .store
        .change_password(
            &account.person_id,
            &body.current_password,
            &body.new_password,
        )
        .await
    {
        Ok(()) => {
            state
                .store
                .record_audit(
                    &account.person_id,
                    None,
                    "password_changed",
                    None,
                    serde_json::json!({}),
                )
                .await?;
            Ok(StatusCode::NO_CONTENT)
        }
        // The password dialog shows this exact text for a wrong current password.
        Err(StoreError::Forbidden(_)) => Err(ApiError::invalid_credentials()),
        Err(error) => Err(error.into()),
    }
}

#[derive(Deserialize)]
pub struct TraceSetting {
    enabled: bool,
}

/// The signed-in person's own search trace. A device token cannot read it.
pub async fn trace(State(state): State<AppState>, Web(account): Web) -> ApiResult<Value> {
    Ok(Json(
        state.store.list_retrieval_trace(&account.person_id).await?,
    ))
}

pub async fn set_trace(
    State(state): State<AppState>,
    Web(account): Web,
    Json(body): Json<TraceSetting>,
) -> ApiResult<Value> {
    state
        .store
        .set_retrieval_trace(&account.person_id, body.enabled)
        .await?;
    Ok(Json(
        state.store.list_retrieval_trace(&account.person_id).await?,
    ))
}
