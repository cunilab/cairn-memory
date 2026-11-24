use crate::AppState;
use crate::auth::Admin;
use crate::error::ApiResult;
use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
pub struct Filter {
    status: Option<String>,
    detector: Option<String>,
}
#[derive(Deserialize)]
pub struct Apply {
    fingerprint: String,
}
#[derive(Deserialize)]
pub struct Rewrite {
    title: String,
    summary: String,
    reason: String,
    fields: Option<Value>,
}

pub async fn list(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Query(filter): Query<Filter>,
) -> ApiResult<Vec<Value>> {
    Ok(Json(
        state
            .store
            .list_fixes(
                &admin.person_id,
                filter.status.as_deref(),
                filter.detector.as_deref(),
            )
            .await?,
    ))
}
pub async fn count(State(state): State<AppState>, Admin(_): Admin) -> ApiResult<Value> {
    Ok(Json(
        json!({"count": state.store.pending_fix_count().await?}),
    ))
}
pub async fn detail(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    Ok(Json(state.store.fix_detail(&admin.person_id, &id).await?))
}
pub async fn apply(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
    Json(body): Json<Apply>,
) -> ApiResult<Value> {
    state
        .store
        .apply_fix(
            &id,
            &body.fingerprint,
            &admin.person_id,
            state.embeddings.is_some(),
            env!("CARGO_PKG_VERSION"),
        )
        .await?;
    Ok(Json(state.store.fix_detail(&admin.person_id, &id).await?))
}
async fn transition(
    state: AppState,
    admin: String,
    id: String,
    operation: &str,
) -> ApiResult<Value> {
    state.store.transition_fix(&id, &admin, operation).await?;
    Ok(Json(state.store.fix_detail(&admin, &id).await?))
}
pub async fn ignore(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    transition(state, admin.person_id, id, "ignore").await
}
pub async fn copied(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    transition(state, admin.person_id, id, "copied").await
}
pub async fn done(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
) -> ApiResult<Value> {
    transition(state, admin.person_id, id, "done").await
}
pub async fn searches(State(state): State<AppState>, Admin(_): Admin) -> ApiResult<Vec<Value>> {
    Ok(Json(state.store.recent_searches().await?))
}
pub async fn rewrite(
    State(state): State<AppState>,
    Admin(admin): Admin,
    Path(id): Path<String>,
    Json(body): Json<Rewrite>,
) -> ApiResult<Value> {
    let new = state
        .store
        .rewrite_memory(
            &admin.person_id,
            &id,
            body.title,
            body.summary,
            body.reason,
            body.fields,
        )
        .await?;
    Ok(Json(json!({"memory_id":new,"superseded":id})))
}
