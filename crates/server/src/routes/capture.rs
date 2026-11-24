use crate::AppState;
use crate::auth::{Agent, Device, DeviceCaller};
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
    nudge_asked: Option<bool>,
    nudge_saved: Option<bool>,
}

/// A typed capture. Unknown keys are a 400, so a client learns about a misnamed field.
pub async fn observe(
    state: State<AppState>,
    device: DeviceCaller,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let object = body
        .as_object_mut()
        .ok_or_else(|| ApiError::invalid("invalid capture"))?;
    let class = object
        .remove("hook_error_class")
        .and_then(|v| v.as_str().map(str::to_owned));
    let at = object.remove("hook_error_at").and_then(|v| v.as_i64());
    let depth = object
        .remove("outbox_depth")
        .and_then(|v| v.as_i64())
        .and_then(|n| i32::try_from(n).ok())
        .filter(|n| *n >= 0);
    if class
        .as_deref()
        .is_some_and(|c| !matches!(c, "network" | "auth" | "server" | "parse"))
    {
        return Err(ApiError::invalid("invalid hook error class"));
    }
    if let Some(depth) = depth {
        state
            .store
            .record_hook_health(
                &device.account.person_id,
                &device.device_id,
                class.as_deref(),
                at,
                depth,
            )
            .await?;
    }
    let input = serde_json::from_value(body)
        .map_err(|error| ApiError::invalid(format!("invalid capture: {error}")))?;
    capture(state, device, input).await
}

/// `project_attachment.status` is `attached` when the capture names a project the person belongs
/// to, `attached_by_repository` when the repository matches exactly one membership, else
/// `personal`. The hook and the CLI print a warning for a broken file.
async fn capture(
    State(state): State<AppState>,
    DeviceCaller { account, device_id }: DeviceCaller,
    input: ObservationInput,
) -> Result<Json<Value>, ApiError> {
    let context = input.context.clone();
    let result = state
        .store
        .capture_from(&account.person_id, input, Some(&device_id))
        .await?;
    // After capture, a committed project id has joined. The status uses that membership.
    let resolution = state
        .store
        .resolve_project(&account.person_id, &context)
        .await?;
    let status = if resolution.by_repository {
        "attached_by_repository"
    } else if resolution.project_id.is_some() {
        "attached"
    } else {
        "personal"
    };
    Ok(Json(json!({
        "accepted": result.accepted,
        "duplicate": result.duplicate,
        "event_id": result.event_id,
        "project_attachment": {"status": status},
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
        .end_session_with_nudge(
            &account.person_id,
            &body.session_id,
            body.nudge_asked,
            body.nudge_saved,
        )
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

#[derive(Deserialize)]
pub struct Lookup {
    session_id: String,
    kind: String,
    query: Option<String>,
    result_ids: Vec<String>,
    hit: Option<bool>,
}

pub async fn lookup(
    State(state): State<AppState>,
    DeviceCaller { account, device_id }: DeviceCaller,
    Json(body): Json<Lookup>,
) -> Result<axum::http::StatusCode, ApiError> {
    if !matches!(body.kind.as_str(), "search" | "get") {
        return Err(ApiError::invalid("invalid lookup kind"));
    }
    let query = body
        .query
        .as_deref()
        .map(cairn_core::capture::redact_sensitive_content)
        .map(|q| q.chars().take(120).collect::<String>());
    let ids: Vec<String> = body.result_ids.into_iter().take(20).collect();
    state
        .store
        .record_lookup(
            (&account.person_id, &device_id),
            &body.session_id,
            &body.kind,
            query.as_deref(),
            &ids,
            body.hit,
        )
        .await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
