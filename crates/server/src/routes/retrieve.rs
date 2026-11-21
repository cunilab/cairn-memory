use crate::AppState;
use crate::auth::Agent;
use crate::error::{ApiError, ApiResult};
use axum::Json;
use axum::extract::{Query, State};
use cairn_core::capture::WorkContext;
use cairn_core::memory::{Kind, Relation, Scope};
use cairn_core::retrieve::brief::SESSION_BRIEF_MAX_BYTES;
use cairn_core::retrieve::filter::Filter;
use cairn_core::retrieve::graph::{Graph, GraphFilter};
use cairn_core::retrieve::pack::Cards;
use cairn_store::export::Export;
use cairn_store::retrieve::Diagnostics;
use serde::Deserialize;
use serde_json::{Value, json};

const MAX_LIMIT: usize = 20;

fn five() -> usize {
    5
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Search {
    query: String,
    #[serde(default)]
    kind: Option<Kind>,
    #[serde(default = "five")]
    limit: usize,
    /// Tokens; accepted and ignored until step 5.
    #[serde(default)]
    #[allow(dead_code)]
    budget: Option<usize>,
    #[serde(default)]
    context: Option<WorkContext>,
}

#[derive(Deserialize)]
pub struct Brief {
    session_id: String,
    #[serde(default)]
    context: Option<WorkContext>,
    #[serde(default)]
    recent_files: Vec<String>,
}

#[derive(Deserialize)]
pub struct Topic {
    path: String,
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

/// The query vector, when the embedding model is loaded and free; otherwise lexical and entity
/// recall carry the search.
async fn vector(state: &AppState, query: &str) -> Option<Vec<f32>> {
    match &state.embeddings {
        Some(model) => model.query(query).await,
        None => None,
    }
}

/// `/search`: cards, not rows.
pub async fn search(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Search>,
) -> ApiResult<Cards> {
    let vector = vector(&state, &body.query).await;
    let filter = Filter {
        kind: body.kind,
        context: body.context,
        ..Filter::default()
    };
    Ok(Json(
        state
            .store
            .search(
                &account.person_id,
                &body.query,
                vector.as_deref(),
                body.limit.clamp(1, MAX_LIMIT),
                &filter,
            )
            .await?,
    ))
}

pub async fn brief(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Brief>,
) -> Result<Json<Value>, ApiError> {
    let context = body.context.unwrap_or_default();
    state
        .store
        .record_session_files(
            &account.person_id,
            &body.session_id,
            &context,
            &body.recent_files,
        )
        .await?;
    let brief = state.store.brief(&account.person_id, &context).await?;
    Ok(Json(
        json!({"brief": brief, "max_bytes": SESSION_BRIEF_MAX_BYTES}),
    ))
}

/// Notes whose file entity is exactly `path`, at most two; never counted as a fetch.
pub async fn topic(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Topic>,
) -> Result<Json<Value>, ApiError> {
    if body.path.trim().is_empty() {
        return Err(ApiError::invalid("path must not be empty"));
    }
    let notes = state
        .store
        .topic_notes(
            &account.person_id,
            &body.path,
            &body.context.unwrap_or_default(),
        )
        .await?;
    Ok(Json(json!({"notes": notes})))
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
