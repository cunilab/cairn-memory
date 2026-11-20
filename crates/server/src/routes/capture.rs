use crate::AppState;
use crate::auth::Agent;
use crate::error::{ApiError, ApiResult};
use axum::Json;
use axum::extract::State;
use cairn_core::capture::{CaptureResult, ObservationInput, WorkContext};
use serde::Deserialize;
use serde_json::{Value, json};

/// A capture. Keys the old hook sent beyond these (`importance`, `lifecycle`) are ignored.
#[derive(Deserialize)]
pub struct Observation {
    #[serde(rename = "eventId", alias = "event_id")]
    event_id: String,
    #[serde(rename = "sessionId", alias = "session_id")]
    session_id: String,
    content: String,
    role: String,
    source: String,
    #[serde(default)]
    context: WorkContext,
}

#[derive(Deserialize)]
pub struct SessionStart {
    #[serde(rename = "sessionId", alias = "session_id")]
    session_id: String,
    #[serde(default)]
    context: WorkContext,
}

#[derive(Deserialize)]
pub struct SessionRef {
    #[serde(rename = "sessionId", alias = "session_id")]
    session_id: String,
}

pub async fn observe(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Observation>,
) -> ApiResult<CaptureResult> {
    let input = ObservationInput {
        event_id: body.event_id,
        session_id: body.session_id,
        content: body.content,
        role: body.role,
        source: body.source,
        context: body.context,
    };
    Ok(Json(state.store.capture(&account.person_id, input).await?))
}

pub async fn start(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<SessionStart>,
) -> Result<Json<Value>, ApiError> {
    state
        .store
        .start_session(&account.person_id, &body.session_id, &body.context)
        .await?;
    Ok(Json(
        json!({"session_id": body.session_id, "status": "started"}),
    ))
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
