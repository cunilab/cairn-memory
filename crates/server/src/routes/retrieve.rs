use crate::AppState;
use crate::auth::Agent;
use crate::error::{ApiError, ApiResult};
use axum::Json;
use axum::extract::{Query, State};
use cairn_core::capture::WorkContext;
use cairn_core::memory::{Relation, Scope};
use cairn_core::retrieve::brief::SESSION_BRIEF_MAX_BYTES;
use cairn_core::retrieve::filter::Filter;
use cairn_core::retrieve::graph::{Graph, GraphFilter};
use cairn_core::retrieve::pack::Pack;
use cairn_store::export::Export;
use cairn_store::retrieve::Diagnostics;
use serde::Deserialize;
use serde_json::{Value, json};

const MAX_LIMIT: usize = 100;

fn default_limit() -> usize {
    10
}

#[derive(Deserialize)]
pub struct Search {
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(rename = "sessionId", alias = "session_id", default)]
    session_id: Option<String>,
    #[serde(default)]
    context: Option<WorkContext>,
}

#[derive(Deserialize)]
pub struct Continuity {
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(rename = "sessionId", alias = "session_id", default)]
    session_id: Option<String>,
    #[serde(default)]
    context: Option<WorkContext>,
}

#[derive(Deserialize)]
pub struct Brief {
    #[serde(default)]
    context: Option<WorkContext>,
}

#[derive(Deserialize)]
pub struct GraphQuery {
    scope: Option<Scope>,
    #[serde(rename = "projectId", alias = "project_id")]
    project_id: Option<String>,
    relation: Option<Relation>,
    cap: Option<usize>,
}

fn limit(requested: usize) -> usize {
    requested.clamp(1, MAX_LIMIT)
}

fn filter(session_id: Option<String>, context: Option<WorkContext>) -> Filter {
    Filter {
        session_id,
        context,
    }
}

/// The query vector, when the embedding model is loaded and free; otherwise lexical and entity
/// recall carry the search.
async fn vector(state: &AppState, query: &str) -> Option<Vec<f32>> {
    match &state.embeddings {
        Some(model) => model.query(query).await,
        None => None,
    }
}

/// `/search`, `/smart-search`, `/context` and `/recall` are one search.
pub async fn search(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Search>,
) -> ApiResult<Pack> {
    let vector = vector(&state, &body.query).await;
    Ok(Json(
        state
            .store
            .search(
                &account.person_id,
                &body.query,
                vector.as_deref(),
                limit(body.limit),
                &filter(body.session_id, body.context),
            )
            .await?,
    ))
}

pub async fn why_changed(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Search>,
) -> ApiResult<Pack> {
    let vector = vector(&state, &body.query).await;
    Ok(Json(
        state
            .store
            .why_changed(
                &account.person_id,
                &body.query,
                vector.as_deref(),
                limit(body.limit),
                &filter(body.session_id, body.context),
            )
            .await?,
    ))
}

pub async fn resume(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Continuity>,
) -> ApiResult<Pack> {
    Ok(Json(
        state
            .store
            .resume(
                &account.person_id,
                limit(body.limit),
                &filter(body.session_id, body.context),
            )
            .await?,
    ))
}

pub async fn unresolved(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Continuity>,
) -> ApiResult<Pack> {
    Ok(Json(
        state
            .store
            .unresolved(
                &account.person_id,
                limit(body.limit),
                &filter(body.session_id, body.context),
            )
            .await?,
    ))
}

pub async fn brief(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Brief>,
) -> Result<Json<Value>, ApiError> {
    let brief = state
        .store
        .brief(&account.person_id, &body.context.unwrap_or_default())
        .await?;
    Ok(Json(
        json!({"brief": brief, "max_bytes": SESSION_BRIEF_MAX_BYTES}),
    ))
}

pub async fn graph(
    State(state): State<AppState>,
    Agent(account): Agent,
    Query(query): Query<GraphQuery>,
) -> ApiResult<Graph> {
    let filter = GraphFilter {
        scope: query.scope,
        project_id: query.project_id.filter(|id| !id.is_empty()),
        relation: query.relation,
    };
    Ok(Json(
        state
            .store
            .graph(&account.person_id, &filter, query.cap)
            .await?,
    ))
}

/// The memories and links the person can see.
pub async fn export(State(state): State<AppState>, Agent(account): Agent) -> ApiResult<Export> {
    Ok(Json(state.store.export(&account.person_id).await?))
}

pub async fn diagnose(
    State(state): State<AppState>,
    Agent(account): Agent,
) -> ApiResult<Diagnostics> {
    Ok(Json(state.store.diagnose(&account.person_id).await?))
}
