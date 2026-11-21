use crate::AppState;
use crate::auth::{Agent, Device};
use crate::error::ApiError;
use axum::Json;
use axum::extract::State;
use cairn_core::capture::{CaptureKind, ObservationInput, WorkContext};
use serde::Deserialize;
use serde_json::{Value, json};

/// The Python hook's capture, for `/sync/push` until step 4 chunk 3 deletes it. Keys beyond
/// these (`importance`, `lifecycle`, `source`) are ignored; the role picks the kind.
#[derive(Deserialize)]
pub struct LegacyObservation {
    #[serde(rename = "eventId", alias = "event_id")]
    event_id: String,
    #[serde(rename = "sessionId", alias = "session_id")]
    session_id: String,
    content: String,
    role: String,
    #[serde(default)]
    context: WorkContext,
}

impl From<LegacyObservation> for ObservationInput {
    fn from(old: LegacyObservation) -> Self {
        let kind = match old.role.trim().to_ascii_lowercase().as_str() {
            "user" => CaptureKind::Prompt,
            "assistant" => CaptureKind::Assistant,
            _ => CaptureKind::Command,
        };
        Self {
            event_id: old.event_id,
            session_id: old.session_id,
            kind,
            ok: None,
            content: old.content,
            context: old.context,
        }
    }
}

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

pub async fn sync_push(
    state: State<AppState>,
    device: Device,
    Json(body): Json<LegacyObservation>,
) -> Result<Json<Value>, ApiError> {
    capture(state, device, body.into()).await
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
