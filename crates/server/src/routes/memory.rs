use crate::AppState;
use crate::auth::Agent;
use crate::error::{ApiError, ApiResult};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use cairn_core::capture::WorkContext;
use cairn_core::memory::{Kind, Memory, Record, SaveInput, Scope, Status};
use cairn_store::memory::{ListFilter, Vote};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

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
    #[serde(alias = "memoryId", alias = "memory_id")]
    id: String,
    vote: String,
}

#[derive(Deserialize)]
pub struct GetMany {
    ids: Vec<String>,
}

/// `missing` holds ids that do not exist or that the person cannot see.
#[derive(Serialize)]
pub struct Got {
    memories: Vec<Record>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    missing: Vec<String>,
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
    Ok(Json(state.store.get(&account.person_id, &id).await?))
}

/// The agent read: up to 10 records in request order, each counted as a fetch.
pub async fn get_many(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<GetMany>,
) -> ApiResult<Got> {
    if body.ids.is_empty() || body.ids.len() > 10 || body.ids.iter().any(|id| id.len() > 128) {
        return Err(ApiError::invalid(
            "ids: 1 to 10 ids of at most 128 characters",
        ));
    }
    let mut found: HashMap<String, Memory> = state
        .store
        .memories_by_ids(&account.person_id, &body.ids)
        .await?
        .into_iter()
        .map(|m| (m.id.clone(), m))
        .collect();
    let (mut memories, mut missing) = (Vec::new(), Vec::new());
    let mut seen = HashSet::new();
    for id in body.ids {
        if !seen.insert(id.clone()) {
            continue;
        }
        match found.remove(&id) {
            Some(memory) => memories.push(Record::from(&memory)),
            None => missing.push(id),
        }
    }
    let fetched: Vec<String> = memories.iter().map(|r| r.id.clone()).collect();
    state.store.mark_fetched(&fetched).await?;
    Ok(Json(Got { memories, missing }))
}

pub async fn feedback(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Feedback>,
) -> ApiResult<Value> {
    let vote = match body.vote.as_str() {
        "helpful" => Vote::Helpful,
        "wrong" | "stale" => Vote::Wrong,
        _ => return Err(ApiError::invalid("vote must be helpful or wrong")),
    };
    let status = state
        .store
        .feedback(&account.person_id, &body.id, vote)
        .await?;
    Ok(Json(json!({"id": body.id, "status": status})))
}

/// Forget: the owner's memory is blanked and stays forgotten.
pub async fn forget(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Target>,
) -> ApiResult<Value> {
    let id = erase(&state, &account.person_id, body).await?;
    Ok(Json(json!({"id": id, "status": "forgotten"})))
}

/// The console's delete: the same erase, no body.
pub async fn delete(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Target>,
) -> Result<StatusCode, ApiError> {
    erase(&state, &account.person_id, body).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn erase(state: &AppState, person_id: &str, body: Target) -> Result<String, ApiError> {
    let id = body
        .memory_id
        .ok_or_else(|| ApiError::invalid("id is required"))?;
    state.store.forget(person_id, &id).await?;
    Ok(id)
}
