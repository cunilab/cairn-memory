//! Web-session browse: literal or related search, numbered pages, pin quotas.

use crate::AppState;
use crate::auth::Web;
use crate::error::{ApiError, ApiResult};
use axum::Json;
use axum::extract::{Query, State};
use cairn_core::memory::literal::{self, LiteralMatch};
use cairn_core::memory::{Kind, Scope, Status, TrustLevel};
use cairn_store::console::{ConsoleFilter, ConsoleMatch};
use cairn_store::view::{MemoryView, PinQuota};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct ConsoleQuery {
    q: Option<String>,
    #[serde(rename = "match")]
    mode: Option<String>,
    page: Option<i64>,
    #[serde(rename = "pageSize", alias = "page_size")]
    page_size: Option<i64>,
    sort: Option<String>,
    pinned: Option<bool>,
    scope: Option<Scope>,
    kind: Option<Kind>,
    #[serde(rename = "projectId", alias = "project_id")]
    project_id: Option<String>,
    status: Option<String>,
    trust: Option<TrustLevel>,
    branch: Option<String>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Serialize)]
pub struct ConsoleMemory {
    #[serde(flatten)]
    #[cfg_attr(feature = "ts", ts(flatten))]
    pub view: MemoryView,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub matched: Option<LiteralMatch>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Serialize)]
pub struct ConsolePage {
    pub items: Vec<ConsoleMemory>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub page: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub page_size: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: i64,
    pub quotas: Vec<PinQuota>,
}

/// `GET /cairn-memory/console/memories`. Web session only. Does not count a fetch.
pub async fn memories(
    State(state): State<AppState>,
    Web(account): Web,
    Query(query): Query<ConsoleQuery>,
) -> ApiResult<ConsolePage> {
    if let Some(sort) = query.sort.as_deref()
        && sort != "updated_desc"
    {
        return Err(ApiError::invalid("sort must be updated_desc"));
    }
    let page = query.page.unwrap_or(1);
    if page < 1 {
        return Err(ApiError::invalid("page starts at 1"));
    }
    let page_size = query.page_size.unwrap_or(25);
    if !matches!(page_size, 25 | 50 | 100) {
        return Err(ApiError::invalid("page size must be 25, 50 or 100"));
    }
    let mode = match query.mode.as_deref() {
        None | Some("") | Some("exact") => ConsoleMatch::Exact,
        Some("related") => ConsoleMatch::Related,
        Some(_) => return Err(ApiError::invalid("match must be exact or related")),
    };
    let q = match query.q.as_deref() {
        Some(raw) if !raw.trim().is_empty() => Some(
            literal::check_query(raw)
                .map_err(ApiError::invalid)?
                .to_owned(),
        ),
        _ => None,
    };
    let mode = if q.is_none() {
        ConsoleMatch::Browse
    } else {
        mode
    };
    let status = query.status.as_deref().filter(|status| !status.is_empty());
    let statuses = match status {
        None => Vec::new(),
        Some(status) => vec![status.parse::<Status>().map_err(ApiError::invalid)?],
    };
    let rows = state
        .store
        .console_memories(
            &account.person_id,
            &ConsoleFilter {
                project_id: query.project_id.filter(|id| !id.is_empty()),
                scope: query.scope,
                kind: query.kind,
                statuses,
                trust: query.trust,
                branch: query.branch.filter(|branch| !branch.is_empty()),
                q: q.clone(),
                mode,
                pinned: query.pinned.unwrap_or(false),
                page,
                page_size,
                see_forgotten: account.is_admin && status == Some("forgotten"),
                include_archived: true,
            },
        )
        .await?;
    let needle = q.as_deref().unwrap_or("");
    let matched: Vec<Option<LiteralMatch>> = rows
        .memories
        .iter()
        .map(|memory| {
            if needle.is_empty() {
                None
            } else {
                literal::best_match(
                    &literal::memory_parts(
                        &memory.title,
                        &memory.summary,
                        &memory.reason,
                        &memory.detail,
                        &memory.fields,
                    ),
                    needle,
                )
            }
        })
        .collect();
    let views = state
        .store
        .memory_views(&account.person_id, rows.memories)
        .await?;
    let items = views
        .into_iter()
        .zip(matched)
        .map(|(view, matched)| ConsoleMemory { view, matched })
        .collect();
    Ok(Json(ConsolePage {
        items,
        page: rows.page,
        page_size: rows.page_size,
        total: rows.total,
        quotas: rows.quotas,
    }))
}
