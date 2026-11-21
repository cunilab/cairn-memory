use crate::AppState;
use crate::auth::{Agent, Device};
use crate::error::ApiError;
use axum::Json;
use axum::extract::State;
use cairn_core::capture::{ObservationInput, WorkContext};
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
pub struct SessionRef {
    #[serde(rename = "sessionId", alias = "session_id")]
    session_id: String,
}

/// `project_attachment.status` is `attached` when the capture's project id names a project the
/// person belongs to, else `personal`. The hook and the CLI print a warning for a broken file.
pub async fn observe(
    State(state): State<AppState>,
    Device(account): Device,
    Json(body): Json<Observation>,
) -> Result<Json<Value>, ApiError> {
    let project_id = body.context.project_id.clone();
    let input = ObservationInput {
        event_id: body.event_id,
        session_id: body.session_id,
        content: body.content,
        role: body.role,
        source: body.source,
        context: body.context,
    };
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
