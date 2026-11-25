use crate::AppState;
use crate::auth::Agent;
use crate::error::{ApiError, ApiResult};
use axum::Json;
use axum::extract::{Query, State};
use cairn_core::capture::WorkContext;
use cairn_core::memory::{Kind, Relation, Scope};
use cairn_core::retrieve::brief::SESSION_BRIEF_MAX_BYTES;
use cairn_core::retrieve::command::ERROR_TAIL_MAX_BYTES;
use cairn_core::retrieve::filter::Filter;
use cairn_core::retrieve::graph::{Graph, GraphFilter};
use cairn_core::retrieve::pack::{BUDGET_DEFAULT, Cards};
use cairn_store::usage::Lookup;
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
    /// Tokens (about bytes / 4), clamped to 200..=4000; default 1,200.
    #[serde(default)]
    budget: Option<usize>,
    #[serde(default)]
    context: Option<WorkContext>,
    /// Calibration only: honoured when the server runs with `CAIRN_DEBUG_SCORES=1`.
    #[serde(default)]
    debug: bool,
}

#[derive(Deserialize)]
pub struct Brief {
    session_id: Option<String>,
    #[serde(default)]
    context: Option<WorkContext>,
    #[serde(default)]
    recent_files: Vec<String>,
}

#[derive(Deserialize)]
pub struct Topic {
    path: String,
    session_id: Option<String>,
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
) -> Result<Json<Value>, ApiError> {
    let vector = vector(&state, &body.query).await;
    let filter = Filter {
        kind: body.kind,
        context: body.context,
        ..Filter::default()
    };
    if body.debug && state.debug_scores {
        let trace = state
            .store
            .score_trace(&account.person_id, &body.query, vector.as_deref(), &filter)
            .await?;
        return Ok(Json(json!({"trace": trace})));
    }
    let cards: Cards = state
        .store
        .search(
            &account.person_id,
            &body.query,
            vector.as_deref(),
            body.limit.clamp(1, MAX_LIMIT),
            body.budget.unwrap_or(BUDGET_DEFAULT),
            &filter,
        )
        .await?;
    let reply = json!(cards);
    record(
        &state,
        &account.person_id,
        Lookup::Search,
        &reply,
        !cards.results.is_empty(),
    )
    .await;
    Ok(Json(reply))
}

/// Counts one lookup for the dashboard: how big the answer was and whether it found anything.
/// A failure to count never fails the lookup.
async fn record(state: &AppState, person_id: &str, lookup: Lookup, reply: &Value, hit: bool) {
    let bytes = reply.to_string().len();
    if let Err(error) = state
        .store
        .record_retrieval(person_id, lookup, bytes, hit)
        .await
    {
        eprintln!("retrieval usage not recorded: {error}");
    }
}

fn push_session_id(id: Option<&str>) -> Result<Option<&str>, ApiError> {
    if id.is_some_and(|id| id.trim().is_empty() || id.len() > 512 || id.contains('\0')) {
        return Err(ApiError::invalid(
            "session_id must be 1–512 bytes with no null bytes",
        ));
    }
    Ok(id)
}

pub async fn brief(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Brief>,
) -> Result<Json<Value>, ApiError> {
    let context = body.context.unwrap_or_default();
    let resolved = state
        .store
        .resolve_project(&account.person_id, &context)
        .await?;
    let session_id = push_session_id(body.session_id.as_deref())?;
    if let Some(session_id) = session_id {
        state
            .store
            .record_session_files(&account.person_id, session_id, &context, &body.recent_files)
            .await?;
    }
    let check = match (&context.project_id, &context.branch) {
        (Some(project), Some(branch)) => {
            state
                .store
                .brief_check(&account.person_id, project, branch)
                .await?
        }
        _ => None,
    };
    // The texts the hook appends at start and sends at Stop, so it needs no other request.
    let instructions = state
        .store
        .effective_instructions(&account.person_id, context.project_id.as_deref())
        .await?;
    let mut reply = json!({
        "max_bytes": SESSION_BRIEF_MAX_BYTES,
        "instructions": {
            "session_instruction": instructions.session_instruction.text,
            "stop_nudge": instructions.stop_nudge.text,
        },
    });
    if let Some(check) = check {
        reply["check"] = json!(check);
    }
    if let Some(project_id) = resolved.project_id {
        if let Some(name) = state
            .store
            .project_name(&account.person_id, &project_id)
            .await?
        {
            reply["project_name"] = json!(name);
        }
        reply["project_id"] = json!(project_id);
    }
    let brief = state
        .store
        .brief(&account.person_id, &context, &body.recent_files, session_id)
        .await?;
    let hit = !brief.text.is_empty();
    reply["brief"] = json!(brief.text);
    record(&state, &account.person_id, Lookup::Brief, &reply, hit).await;
    Ok(Json(reply))
}

/// Notes matching a file, its module or a touched symbol. Never counted as a fetch.
pub async fn topic(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Topic>,
) -> Result<Json<Value>, ApiError> {
    if body.path.trim().is_empty() {
        return Err(ApiError::invalid("path must not be empty"));
    }
    let topic = state
        .store
        .topic(
            &account.person_id,
            &body.path,
            &body.context.unwrap_or_default(),
            push_session_id(body.session_id.as_deref())?,
        )
        .await?;
    let reply = json!(topic);
    record(
        &state,
        &account.person_id,
        Lookup::Topic,
        &reply,
        !topic.notes.is_empty() || topic.card.is_some(),
    )
    .await;
    Ok(Json(reply))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRecall {
    session_id: String,
    project_id: Option<String>,
    command: String,
    error_tail: Option<String>,
}

pub async fn command_recall(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<CommandRecall>,
) -> ApiResult<Value> {
    push_session_id(Some(&body.session_id))?;
    if body.command.len() > 800
        || body
            .error_tail
            .as_ref()
            .is_some_and(|tail| tail.len() > ERROR_TAIL_MAX_BYTES)
    {
        return Err(ApiError::invalid(
            "command recall input exceeds its byte limit",
        ));
    }
    let recall = state.store.command_recall(
        &account.person_id,
        &body.session_id,
        body.project_id.as_deref(),
        &body.command,
        body.error_tail.as_deref(),
    );
    let text = match tokio::time::timeout(std::time::Duration::from_millis(750), recall).await {
        Ok(result) => result?,
        Err(_) => String::new(),
    };
    Ok(Json(json!({"text": text})))
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
