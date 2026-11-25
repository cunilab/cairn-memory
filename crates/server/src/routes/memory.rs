use crate::AppState;
use crate::auth::{Agent, Caller, Web};
use crate::error::{ApiError, ApiResult};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use cairn_core::capture::WorkContext;
use cairn_core::memory::{Kind, Memory, Record, SaveInput, Scope, Status, TrustLevel};
use cairn_store::memory::{ListFilter, Saved, Vote};
use cairn_store::transfer::ImportReport;
use cairn_store::usage::Lookup;
use cairn_store::view::MemoryView;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

#[derive(Serialize)]
pub struct Receipt {
    id: String,
    status: Status,
    deduplicated: bool,
    #[serde(flatten)]
    coaching: Option<Coaching>,
}

#[derive(Serialize)]
struct Coaching {
    hints: Vec<String>,
    improve: String,
}

impl Coaching {
    fn from_saved(saved: &mut Saved) -> Option<Self> {
        (!saved.hints.is_empty()).then(|| Self {
            hints: std::mem::take(&mut saved.hints),
            improve: format!("Improve it: save again with supersedes={}.", saved.id),
        })
    }
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
    #[serde(default)]
    detail: bool,
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
    /// A memory status. `forgotten` is listed only for an admin.
    status: Option<String>,
    trust: Option<TrustLevel>,
    branch: Option<String>,
    /// A web search over title, summary, reason and fields.
    q: Option<String>,
}

#[derive(Deserialize)]
pub struct ImportQuery {
    /// `1` or `true`: report what would happen and write nothing.
    dry_run: Option<String>,
}

/// One note in a batch save. `id` is absent when the note was not stored.
#[derive(Serialize)]
struct ItemResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(flatten)]
    coaching: Option<Coaching>,
}

#[derive(Serialize)]
struct ItemResults {
    results: Vec<ItemResult>,
}

/// The body is a `SaveInput`, plus an optional `context` (the client's git and project state),
/// or `items` (at most 3 of those notes and no top-level note fields). Unknown keys are a 400,
/// so an agent learns about a misnamed field instead of losing it.
pub async fn remember(
    State(state): State<AppState>,
    caller: Caller,
    Json(mut body): Json<Value>,
) -> Result<Response, ApiError> {
    let account = caller.account;
    let context = match body
        .as_object_mut()
        .and_then(|object| object.remove("context"))
    {
        None | Some(Value::Null) => WorkContext::default(),
        Some(value) => {
            serde_json::from_value(value).map_err(|e| ApiError::invalid(format!("context: {e}")))?
        }
    };
    if body.get("items").is_some() {
        return save_items(
            &state,
            &account.person_id,
            caller.device_id.as_deref(),
            &context,
            body,
        )
        .await;
    }
    let input: SaveInput =
        serde_json::from_value(body).map_err(|e| ApiError::invalid(e.to_string()))?;
    let mut saved = state
        .store
        .save_from(
            &account.person_id,
            input,
            &context,
            caller.device_id.as_deref(),
        )
        .await?;
    Ok(Json(Receipt {
        coaching: Coaching::from_saved(&mut saved),
        id: saved.id,
        status: saved.status,
        deduplicated: saved.deduplicated,
    })
    .into_response())
}

/// Each item is saved on its own. A bad item is one `error` result; the others still save.
async fn save_items(
    state: &AppState,
    person_id: &str,
    device_id: Option<&str>,
    context: &WorkContext,
    body: Value,
) -> Result<Response, ApiError> {
    let mut object = match body {
        Value::Object(object) => object,
        _ => return Err(ApiError::invalid("body must be an object")),
    };
    let items = object
        .remove("items")
        .ok_or_else(|| ApiError::invalid("items must be a list of at most 3 notes"))?;
    if !object.is_empty() {
        let names = object.keys().cloned().collect::<Vec<_>>().join(", ");
        return Err(ApiError::invalid(format!(
            "items cannot be combined with {names}"
        )));
    }
    let Some(items) = items.as_array() else {
        return Err(ApiError::invalid("items must be a list of at most 3 notes"));
    };
    if items.len() > 3 {
        return Err(ApiError::invalid("items: at most 3 notes"));
    }
    let mut results = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let mut result = save_item(state, person_id, device_id, context, item).await;
        result.error = result.error.map(|error| format!("items[{index}]: {error}"));
        results.push(result);
    }
    Ok(Json(ItemResults { results }).into_response())
}

async fn save_item(
    state: &AppState,
    person_id: &str,
    device_id: Option<&str>,
    context: &WorkContext,
    item: &Value,
) -> ItemResult {
    let input = match serde_json::from_value::<SaveInput>(item.clone()) {
        Ok(input) => input,
        Err(error) => {
            return ItemResult {
                id: None,
                status: "error",
                error: Some(error.to_string()),
                coaching: None,
            };
        }
    };
    match state
        .store
        .save_from(person_id, input, context, device_id)
        .await
    {
        Ok(mut saved) => ItemResult {
            coaching: Coaching::from_saved(&mut saved),
            id: Some(saved.id),
            status: if saved.deduplicated {
                "duplicate"
            } else {
                "saved"
            },
            error: None,
        },
        Err(error) => ItemResult {
            id: None,
            status: "error",
            error: Some(error.to_string()),
            coaching: None,
        },
    }
}

/// Newest update first. The next cursor, when there is one, is in the `x-next-cursor` header.
pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Query(query): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let account = caller.account;
    let status = query.status.as_deref().filter(|status| !status.is_empty());
    let statuses = match status {
        None => Vec::new(),
        Some(status) => vec![status.parse::<Status>().map_err(ApiError::invalid)?],
    };
    let page = state
        .store
        .list(
            &account.person_id,
            &ListFilter {
                project_id: query.project_id,
                scope: query.scope,
                kind: query.kind,
                statuses,
                trust: query.trust,
                limit: query.limit,
                cursor: query.cursor,
                branch: query.branch.filter(|b| !b.is_empty()),
                q: query.q,
                see_forgotten: account.is_admin && status == Some("forgotten"),
                include_archived: caller.device_id.is_none(),
            },
        )
        .await?;
    let mut headers = HeaderMap::new();
    if let Some(cursor) = page.next_cursor
        && let Ok(value) = HeaderValue::from_str(&cursor)
    {
        headers.insert("x-next-cursor", value);
    }
    let views = state
        .store
        .memory_views(&account.person_id, page.memories)
        .await?;
    Ok((headers, Json(views)).into_response())
}

/// One memory for the console; never counts as a fetch.
pub async fn get(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<MemoryView> {
    let account = caller.account;
    let mut memory = state
        .store
        .get_visible(&account.person_id, &id, caller.device_id.is_none())
        .await?;
    let detail = (!memory.detail.is_empty()).then(|| std::mem::take(&mut memory.detail));
    let mut views = state
        .store
        .memory_views(&account.person_id, vec![memory])
        .await?;
    let mut view = views.remove(0);
    view.detail = detail;
    Ok(Json(view))
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
    if body.detail && body.ids.len() != 1 {
        return Err(ApiError::invalid("request detail for one memory at a time"));
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
            Some(memory) => {
                let mut record = Record::from(&memory);
                if body.detail {
                    record.detail = Some(memory.detail);
                }
                memories.push(record);
            }
            None => missing.push(id),
        }
    }
    let fetched: Vec<String> = memories.iter().map(|r| r.id.clone()).collect();
    state.store.mark_agent_read(&fetched, body.detail).await?;
    let reply = Got { memories, missing };
    let bytes = serde_json::to_string(&reply).map_or(0, |text| text.len());
    state
        .store
        .record_retrieval(
            &account.person_id,
            Lookup::Get,
            bytes,
            !reply.memories.is_empty(),
        )
        .await
        .unwrap_or_else(|error| eprintln!("retrieval usage not recorded: {error}"));
    Ok(Json(reply))
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
    let mut reply = json!({"id": body.id, "status": status});
    // A vote cannot lift a stale that came from the repo check: say why it is still stale.
    if status == Status::Stale {
        let memory = state.store.get(&account.person_id, &body.id).await?;
        if let Some(cause) = memory.stale_cause {
            reply["stale_cause"] = json!(cause);
        }
    }
    Ok(Json(reply))
}

/// Forget: the owner's memory stays, hidden from search, with its text kept.
pub async fn forget(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<Target>,
) -> ApiResult<Value> {
    let id = erase(&state, &caller, body).await?;
    Ok(Json(json!({"id": id, "status": "forgotten"})))
}

async fn erase(state: &AppState, caller: &Caller, body: Target) -> Result<String, ApiError> {
    let id = body
        .memory_id
        .ok_or_else(|| ApiError::invalid("id is required"))?;
    if state.store.forget(&caller.account.person_id, &id).await? {
        state
            .store
            .record_audit(
                &caller.account.person_id,
                caller.device_id.as_deref(),
                "memory_forgotten",
                Some(&id),
                json!({}),
            )
            .await?;
    }
    Ok(id)
}

/// The memories the person owns, as a download. A console action: a device token cannot.
pub async fn export(
    State(state): State<AppState>,
    Web(account): Web,
) -> Result<Response, ApiError> {
    let export = state.store.export_owned(&account.person_id).await?;
    let body = serde_json::to_vec(&export).map_err(|e| ApiError::invalid(e.to_string()))?;
    let disposition = format!(
        "attachment; filename=\"cairn-export-{}.json\"",
        civil_date(export.exported_at)
    );
    Ok((
        [
            (header::CONTENT_TYPE, "application/json".to_owned()),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        body,
    )
        .into_response())
}

/// Saves what an export carries, through the normal agent save. `?dry_run=1` only reports.
pub async fn import(
    State(state): State<AppState>,
    Web(account): Web,
    Query(query): Query<ImportQuery>,
    Json(body): Json<Value>,
) -> ApiResult<ImportReport> {
    if body["version"] != 1 {
        return Err(ApiError::invalid("version must be 1"));
    }
    let items = body["memories"]
        .as_array()
        .ok_or_else(|| ApiError::invalid("memories must be a list"))?;
    let dry_run = matches!(query.dry_run.as_deref(), Some("1" | "true"));
    let report = state
        .store
        .import_memories(&account.person_id, items, dry_run)
        .await?;
    if !dry_run && report.new > 0 {
        state
            .store
            .record_audit(
                &account.person_id,
                None,
                "memories_imported",
                None,
                serde_json::json!({"new": report.new, "duplicate": report.duplicate, "invalid": report.invalid.len()}),
            )
            .await?;
    }
    Ok(Json(report))
}

/// `YYYY-MM-DD` (UTC) of epoch seconds.
fn civil_date(seconds: i64) -> String {
    let z = seconds.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::civil_date;

    #[test]
    fn dates_come_out_in_utc() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(951_782_400), "2000-02-29");
        assert_eq!(civil_date(1_795_392_000 + 86_399), "2026-11-23");
    }
}
