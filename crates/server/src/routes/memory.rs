use crate::AppState;
use crate::auth::Agent;
use crate::error::{ApiError, ApiResult};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use cairn_core::capture::WorkContext;
use cairn_core::memory::{Kind, Memory, Relation, SaveInput, Scope, Status};
use cairn_store::memory::{ListFilter, Vote};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize)]
pub struct Receipt {
    id: String,
    status: Status,
    deduplicated: bool,
}

/// An id, under any of the names the console, the CLI and the hook use.
#[derive(Deserialize)]
pub struct Target {
    #[serde(
        alias = "id",
        alias = "memoryId",
        alias = "targetId",
        alias = "target_id"
    )]
    memory_id: Option<String>,
}

#[derive(Deserialize)]
pub struct Feedback {
    #[serde(rename = "memoryId", alias = "memory_id", alias = "id")]
    memory_id: String,
    vote: String,
}

#[derive(Deserialize)]
pub struct Supersede {
    #[serde(rename = "successorId", alias = "successor_id")]
    successor_id: String,
    #[serde(rename = "targetId", alias = "target_id")]
    target_id: String,
}

#[derive(Deserialize)]
pub struct Contradict {
    #[serde(rename = "contradictingId", alias = "contradicting_id")]
    contradicting_id: String,
    #[serde(rename = "contradictedId", alias = "contradicted_id")]
    contradicted_id: String,
}

#[derive(Deserialize)]
pub struct LinkBody {
    #[serde(rename = "sourceId", alias = "source_id")]
    source_id: String,
    relation: Relation,
    #[serde(rename = "targetId", alias = "target_id")]
    target_id: String,
}

#[derive(Deserialize)]
pub struct ListQuery {
    limit: Option<i64>,
    cursor: Option<String>,
    scope: Option<Scope>,
    kind: Option<Kind>,
    #[serde(rename = "projectId", alias = "project_id")]
    project_id: Option<String>,
    status: Option<Status>,
}

/// The body is a `SaveInput`, plus an optional `context` (the client's git and project state).
/// Unknown keys are a 400, so an agent learns about a misnamed field instead of losing it.
pub async fn remember(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(mut body): Json<Value>,
) -> ApiResult<Receipt> {
    let context = match body.as_object_mut().and_then(|o| o.remove("context")) {
        None | Some(Value::Null) => WorkContext::default(),
        Some(value) => {
            serde_json::from_value(value).map_err(|e| ApiError::invalid(format!("context: {e}")))?
        }
    };
    let input: SaveInput =
        serde_json::from_value(body).map_err(|e| ApiError::invalid(e.to_string()))?;
    let saved = state
        .store
        .save(&account.person_id, input, &context)
        .await?;
    Ok(Json(Receipt {
        id: saved.id,
        status: saved.status,
        deduplicated: saved.deduplicated,
    }))
}

/// Newest update first. The next cursor, when there is one, is in the `x-next-cursor` header.
pub async fn list(
    State(state): State<AppState>,
    Agent(account): Agent,
    Query(query): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let page = state
        .store
        .list(
            &account.person_id,
            &ListFilter {
                project_id: query.project_id,
                scope: query.scope,
                kind: query.kind,
                statuses: query.status.into_iter().collect(),
                limit: query.limit,
                cursor: query.cursor,
            },
        )
        .await?;
    let mut headers = HeaderMap::new();
    if let Some(cursor) = page.next_cursor
        && let Ok(value) = HeaderValue::from_str(&cursor)
    {
        headers.insert("x-next-cursor", value);
    }
    Ok((headers, Json(page.memories)).into_response())
}

pub async fn get(
    State(state): State<AppState>,
    Agent(account): Agent,
    Path(id): Path<String>,
) -> ApiResult<Memory> {
    let memory = state.store.get(&account.person_id, &id).await?;
    state
        .store
        .mark_fetched(std::slice::from_ref(&memory.id))
        .await?;
    Ok(Json(memory))
}

pub async fn feedback(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Feedback>,
) -> ApiResult<Memory> {
    let vote = match body.vote.as_str() {
        "helpful" => Vote::Helpful,
        "wrong" | "stale" => Vote::Wrong,
        _ => return Err(ApiError::invalid("vote must be helpful or wrong")),
    };
    state
        .store
        .feedback(&account.person_id, &body.memory_id, vote)
        .await?;
    Ok(Json(
        state.store.get(&account.person_id, &body.memory_id).await?,
    ))
}

/// Forget (also the console's delete): the owner's memory is blanked and stays forgotten.
pub async fn forget(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Target>,
) -> Result<StatusCode, ApiError> {
    let id = body
        .memory_id
        .ok_or_else(|| ApiError::invalid("id is required"))?;
    state.store.forget(&account.person_id, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn supersede(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Supersede>,
) -> ApiResult<Memory> {
    linked(
        &state,
        &account.person_id,
        &body.successor_id,
        &body.target_id,
        Relation::Supersedes,
    )
    .await
}

pub async fn contradict(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Contradict>,
) -> ApiResult<Memory> {
    linked(
        &state,
        &account.person_id,
        &body.contradicting_id,
        &body.contradicted_id,
        Relation::Contradicts,
    )
    .await
}

pub async fn link(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<LinkBody>,
) -> ApiResult<Memory> {
    linked(
        &state,
        &account.person_id,
        &body.source_id,
        &body.target_id,
        body.relation,
    )
    .await
}

pub async fn unlink(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<LinkBody>,
) -> ApiResult<Memory> {
    state
        .store
        .remove_link(
            &account.person_id,
            &body.source_id,
            &body.target_id,
            body.relation,
        )
        .await?;
    Ok(Json(
        state.store.get(&account.person_id, &body.source_id).await?,
    ))
}

async fn linked(
    state: &AppState,
    person_id: &str,
    source: &str,
    target: &str,
    relation: Relation,
) -> ApiResult<Memory> {
    state
        .store
        .add_link(person_id, source, target, relation)
        .await?;
    Ok(Json(state.store.get(person_id, source).await?))
}
