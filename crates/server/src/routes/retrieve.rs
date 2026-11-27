use crate::AppState;
use crate::auth::{Agent, Caller};
use crate::error::{ApiError, ApiResult};
use axum::Json;
use axum::extract::{Query, State};
use cairn_core::capture::{ChatContext, WorkContext, redact_sensitive_content};
use cairn_core::memory::{Kind, Relation, Scope};
use cairn_core::retrieve::brief::BriefBudget;
use cairn_core::retrieve::command::ERROR_TAIL_MAX_BYTES;
use cairn_core::retrieve::filter::Filter;
use cairn_core::retrieve::graph::{Graph, GraphFilter};
use cairn_core::retrieve::pack::{BUDGET_DEFAULT, Cards};
use cairn_core::retrieve::prompt::{PROMPT_QUERY_MAX_BYTES, should_recall};
use cairn_store::usage::Lookup;
use serde::Deserialize;
use serde_json::{Value, json};

const MAX_LIMIT: usize = 20;
const RECALL_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(750);

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
    /// Inclusive ISO date or timestamp. Not read from the query text.
    #[serde(default)]
    recorded_from: Option<String>,
    /// Inclusive ISO date or timestamp. A bare date is the end of that UTC day.
    #[serde(default)]
    recorded_to: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    version: Option<String>,
}

#[derive(Deserialize)]
pub struct Brief {
    session_id: Option<String>,
    #[serde(default)]
    context: Option<WorkContext>,
    #[serde(default)]
    recent_files: Vec<String>,
    #[serde(default)]
    extended_brief: bool,
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
    let context = match body.context {
        Some(context) => Some(chat_project(&state, &account.person_id, context).await?),
        None => None,
    };
    let recorded_from = body
        .recorded_from
        .as_deref()
        .map(|value| cairn_core::retrieve::filter::parse_recorded(value, false))
        .transpose()
        .map_err(ApiError::invalid)?;
    let recorded_to = body
        .recorded_to
        .as_deref()
        .map(|value| cairn_core::retrieve::filter::parse_recorded(value, true))
        .transpose()
        .map_err(ApiError::invalid)?;
    if recorded_from.is_some_and(|from| recorded_to.is_some_and(|to| from > to)) {
        return Err(ApiError::invalid(
            "recorded_from must not be after recorded_to",
        ));
    }
    let branch = body
        .branch
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty());
    if branch.is_some_and(|branch| branch.len() > 200 || branch.contains('\0')) {
        return Err(ApiError::invalid("branch must be at most 200 characters"));
    }
    let version = body
        .version
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    if version.is_some_and(|version| version.len() > 40 || version.contains('\0')) {
        return Err(ApiError::invalid("version must be at most 40 characters"));
    }
    let filter = Filter {
        kind: body.kind,
        context,
        recorded_from,
        recorded_to,
        branch: branch.map(str::to_owned),
        version: version.map(str::to_owned),
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
    let hits: Vec<cairn_store::trace::TraceHit> = cards
        .results
        .iter()
        .enumerate()
        .map(|(index, card)| cairn_store::trace::TraceHit {
            id: card.id.clone(),
            rank: (index + 1) as i32,
            channel: card.why.clone().unwrap_or_default(),
        })
        .collect();
    if let Err(error) = state
        .store
        .record_retrieval_trace(&account.person_id, &body.query, &hits)
        .await
    {
        eprintln!("retrieval trace not recorded: {error}");
    }
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

pub(super) fn push_session_id(id: Option<&str>) -> Result<Option<&str>, ApiError> {
    if id.is_some_and(|id| id.trim().is_empty() || id.len() > 512 || id.contains('\0')) {
        return Err(ApiError::invalid(
            "session_id must be 1–512 bytes with no null bytes",
        ));
    }
    Ok(id)
}

pub(super) fn validate_chat_session(id: &str, chat: Option<&ChatContext>) -> Result<(), ApiError> {
    let chat_id = id.strip_prefix("hchat_").is_some_and(|value| {
        value.len() == 32
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    });
    if chat.is_some() != chat_id {
        return Err(ApiError::invalid("invalid Hermes chat session id"));
    }
    Ok(())
}

async fn chat_project(
    state: &AppState,
    person_id: &str,
    mut work: WorkContext,
) -> Result<WorkContext, ApiError> {
    if work.chat.is_some() {
        work.project_id = state
            .store
            .resolve_project(person_id, &work)
            .await?
            .project_id;
    }
    Ok(work)
}

pub async fn brief(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Brief>,
) -> Result<Json<Value>, ApiError> {
    let mut context = body.context.unwrap_or_default();
    let resolved = state
        .store
        .resolve_project(&account.person_id, &context)
        .await?;
    if context.chat.is_some() {
        context.project_id.clone_from(&resolved.project_id);
    }
    let session_id = push_session_id(body.session_id.as_deref())?;
    if let Some(session_id) = session_id {
        validate_chat_session(session_id, context.chat.as_ref())?;
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
    let budget = if body.extended_brief {
        BriefBudget::Extended
    } else {
        BriefBudget::Legacy
    };
    let mut reply = json!({"max_bytes": budget.max_bytes()});
    if context.chat.is_some() {
        reply["instructions"] = json!({
            "chat_instruction": state.store.chat_instruction().await?,
        });
    } else {
        // The texts the hook appends at start and sends at Stop, so it needs no other request.
        let instructions = state
            .store
            .effective_instructions(&account.person_id, context.project_id.as_deref())
            .await?;
        reply["instructions"] = json!({
            "session_instruction": instructions.session_instruction.text,
            "stop_nudge": instructions.stop_nudge.text,
        });
    }
    if let Some(check) = check {
        reply["check"] = json!(check);
    }
    let brief_context = WorkContext {
        project_id: resolved.project_id.clone(),
        ..context
    };
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
        .brief(&account.person_id, &brief_context, session_id, budget)
        .await?;
    let hit = !brief.text.is_empty();
    reply["brief"] = json!(brief.text);
    if brief.pins_not_shown > 0 {
        reply["pins_not_shown"] = json!(brief.pins_not_shown);
    }
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
    let context =
        chat_project(&state, &account.person_id, body.context.unwrap_or_default()).await?;
    let topic = state
        .store
        .topic(
            &account.person_id,
            &body.path,
            &context,
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
    chat: Option<ChatContext>,
}

pub async fn command_recall(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<CommandRecall>,
) -> ApiResult<Value> {
    push_session_id(Some(&body.session_id))?;
    validate_chat_session(&body.session_id, body.chat.as_ref())?;
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
    let work = chat_project(
        &state,
        &account.person_id,
        WorkContext {
            project_id: body.project_id,
            chat: body.chat,
            ..WorkContext::default()
        },
    )
    .await?;
    let recall = state.store.command_recall_in(
        &account.person_id,
        &body.session_id,
        work.project_id.as_deref(),
        &body.command,
        body.error_tail.as_deref(),
        work.chat.as_ref(),
    );
    let text = match tokio::time::timeout(RECALL_TIMEOUT, recall).await {
        Ok(result) => result?,
        Err(_) => String::new(),
    };
    Ok(Json(json!({"text": text})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptRecall {
    session_id: String,
    prompt: String,
    #[serde(default)]
    context: WorkContext,
}

pub async fn prompt_recall(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<PromptRecall>,
) -> ApiResult<Value> {
    push_session_id(Some(&body.session_id))?;
    validate_chat_session(&body.session_id, body.context.chat.as_ref())?;
    if body.prompt.len() > PROMPT_QUERY_MAX_BYTES || body.prompt.contains('\0') {
        return Ok(Json(json!({"text": ""})));
    }
    let prompt = redact_sensitive_content(&body.prompt);
    if !should_recall(&prompt) {
        return Ok(Json(json!({"text": ""})));
    }
    let context = chat_project(&state, &account.person_id, body.context).await?;
    let recall = async {
        if state
            .store
            .push_budget_spent(&account.person_id, &body.session_id)
            .await?
        {
            return Ok::<_, cairn_store::StoreError>((String::new(), true));
        }
        let embedding = vector(&state, &prompt).await;
        state
            .store
            .prompt_recall(
                &account.person_id,
                &body.session_id,
                context,
                &prompt,
                embedding.as_deref(),
            )
            .await
    };
    let (text, spent) = match tokio::time::timeout(RECALL_TIMEOUT, recall).await {
        Ok(result) => result?,
        Err(_) => (String::new(), false),
    };
    Ok(Json(json!({"text": text, "budget_spent": spent})))
}

pub async fn graph(
    State(state): State<AppState>,
    caller: Caller,
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
            .graph(
                &caller.account.person_id,
                &filter,
                query.cap,
                caller.device_id.is_some(),
            )
            .await?,
    ))
}
