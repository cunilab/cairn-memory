use crate::AppState;
use crate::auth::Admin;
use crate::error::{ApiError, ApiResult};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use cairn_store::jobs::JobSnapshot;

pub async fn jobs(State(state): State<AppState>, Admin(_): Admin) -> ApiResult<JobSnapshot> {
    Ok(Json(state.store.job_snapshot().await?))
}

pub async fn requeue(
    State(state): State<AppState>,
    Admin(_): Admin,
    Path(job_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.store.requeue_job(&job_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn discard(
    State(state): State<AppState>,
    Admin(_): Admin,
    Path(job_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.store.discard_job(&job_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
