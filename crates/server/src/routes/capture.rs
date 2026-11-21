use crate::AppState;
use crate::auth::{Agent, Device};
use crate::error::ApiError;
use axum::Json;
use axum::extract::State;
use cairn_core::capture::ObservationInput;
use cairn_store::repo::RepoState;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
pub struct SessionRef {
    #[serde(rename = "sessionId", alias = "session_id")]
    session_id: String,
}

/// A typed capture. Unknown keys are a 400, so a client learns about a misnamed field.
pub async fn observe(
    state: State<AppState>,
    device: Device,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let input = serde_json::from_value(body)
        .map_err(|error| ApiError::invalid(format!("invalid capture: {error}")))?;
    capture(state, device, input).await
}

/// `project_attachment.status` is `attached` when the capture's project id names a project the
/// person belongs to, else `personal`. The hook and the CLI print a warning for a broken file.
async fn capture(
    State(state): State<AppState>,
    Device(account): Device,
    input: ObservationInput,
) -> Result<Json<Value>, ApiError> {
    let project_id = input.context.project_id.clone();
    let result = state.store.capture(&account.person_id, input).await?;
    let attached = match project_id {
        Some(id) => state.store.is_member(&account.person_id, &id).await?,
        None => false,
    };
    Ok(Json(json!({
        "accepted": result.accepted,
        "duplicate": result.duplicate,
        "event_id": result.event_id,
        "project_attachment": {"status": if attached { "attached" } else { "personal" }},
    })))
}

/// Closes the session and queues its distill. `queued: false` means nothing was captured.
pub async fn end(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<SessionRef>,
) -> Result<Json<Value>, ApiError> {
    let queued = state
        .store
        .end_session(&account.person_id, &body.session_id)
        .await?;
    Ok(Json(
        json!({"status": if queued { "queued" } else { "empty" }}),
    ))
}

/// Queues a distill and keeps the session open; the hook retries unless the status is one of
/// `stored`, `unchanged`, `empty`.
pub async fn checkpoint(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<SessionRef>,
) -> Result<Json<Value>, ApiError> {
    let queued = state
        .store
        .checkpoint_session(&account.person_id, &body.session_id)
        .await?;
    Ok(Json(
        json!({"status": if queued { "stored" } else { "unchanged" }}),
    ))
}

/// The hook's answer to the brief's `check`: which cited files and symbols exist on the default
/// branch, and which branches were merged. Replies `{stale, revived, promoted}`.
pub async fn repo_state(
    State(state): State<AppState>,
    Device(account): Device,
    Json(body): Json<RepoState>,
) -> Result<Json<Value>, ApiError> {
    let report = state.store.repo_state(&account.person_id, &body).await?;
    Ok(Json(json!(report)))
}
