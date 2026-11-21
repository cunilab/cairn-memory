use crate::AppState;
use crate::auth::Admin;
use crate::error::ApiResult;
use axum::Json;
use axum::extract::State;
use cairn_store::jobs::JobSnapshot;

/// Read-only: Cairn retries a quarantined job by itself, after each upgrade and once a day.
pub async fn jobs(State(state): State<AppState>, Admin(_): Admin) -> ApiResult<JobSnapshot> {
    Ok(Json(state.store.job_snapshot().await?))
}
