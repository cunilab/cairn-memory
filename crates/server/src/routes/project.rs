use crate::AppState;
use crate::auth::{Agent, Caller, Web};
use crate::error::{ApiError, ApiResult};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use cairn_core::memory::{Scope, Status};
use cairn_core::project::Project;
use cairn_store::StoreError;
use cairn_store::memory::ListFilter;
use cairn_store::project::{BranchSummary, Conflict, SessionReport};
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "ProjectView"))]
#[derive(Serialize)]
pub struct Listed {
    #[serde(flatten)]
    #[cfg_attr(feature = "ts", ts(flatten))]
    project: Project,
    #[cfg_attr(feature = "ts", ts(type = "\"owner\" | \"member\""))]
    role: &'static str,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    memories: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    stale: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    members: i64,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    last_activity_at: Option<i64>,
}

/// A roster row; the console reads `role`.
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "ProjectMember")
)]
#[derive(Serialize)]
pub struct MemberRow {
    person_id: String,
    email: String,
    #[cfg_attr(feature = "ts", ts(type = "\"owner\" | \"member\""))]
    role: &'static str,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    joined_at: i64,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    last_active_at: Option<i64>,
}

#[derive(Deserialize)]
pub struct Init {
    #[serde(default)]
    project_id: Option<String>,
    repository: String,
    #[serde(default)]
    name: Option<String>,
    /// The caller knows the server has never seen `project_id` (a wipe or a new server) and
    /// accepts becoming its owner. Without it an unknown id is refused.
    #[serde(default)]
    confirm_register: bool,
}

#[derive(Serialize)]
pub struct Initialized {
    project_id: String,
    repository: String,
    name: String,
    created: bool,
}

#[derive(Deserialize)]
pub struct Rename {
    name: String,
}

#[derive(Deserialize)]
pub struct Transfer {
    #[serde(rename = "personId", alias = "person_id")]
    person_id: String,
}

#[derive(Deserialize)]
pub struct Paging {
    limit: Option<i64>,
    cursor: Option<String>,
}

pub async fn list(State(state): State<AppState>, Agent(account): Agent) -> ApiResult<Vec<Listed>> {
    let projects = state.store.list_projects(&account.person_id).await?;
    let ids: Vec<String> = projects.iter().map(|p| p.project_id.clone()).collect();
    let mut counts = state.store.project_counts(&ids).await?;
    Ok(Json(
        projects
            .into_iter()
            .map(|project| {
                let c = counts.remove(&project.project_id).unwrap_or_default();
                Listed {
                    role: if project.owner_id == account.person_id {
                        "owner"
                    } else {
                        "member"
                    },
                    memories: c.memories,
                    stale: c.stale,
                    members: c.members,
                    last_activity_at: c.last_activity_at,
                    project,
                }
            })
            .collect(),
    ))
}

/// `cairn init`: with a committed project id the caller joins it; otherwise one is created.
pub async fn init(
    State(state): State<AppState>,
    caller: Caller,
    Json(body): Json<Init>,
) -> ApiResult<Initialized> {
    let account = caller.account;
    let (project, created) = match body.project_id.as_deref() {
        Some(id) => match state
            .store
            .join_project(&account.person_id, id, body.name.as_deref())
            .await
        {
            Ok(project) => (project, false),
            // A wiped or rebuilt server: register the committed id again, owned by the caller.
            // Whoever asks first owns it, so the caller must say they know that.
            Err(StoreError::NotFound(_)) => {
                state
                    .store
                    .restore_project(
                        &account.person_id,
                        id,
                        &body.repository,
                        body.name.as_deref(),
                        body.confirm_register,
                    )
                    .await?
            }
            Err(error) => return Err(error.into()),
        },
        None => {
            state
                .store
                .init_project(&account.person_id, &body.repository, body.name.as_deref())
                .await?
        }
    };
    if created {
        state
            .store
            .record_audit(
                &account.person_id,
                caller.device_id.as_deref(),
                "project_initialized",
                Some(&project.project_id),
                serde_json::json!({"repository": project.repository, "name": project.name}),
            )
            .await?;
    }
    Ok(Json(Initialized {
        project_id: project.project_id,
        repository: project.repository,
        name: project.name,
        created,
    }))
}

pub async fn rename(
    State(state): State<AppState>,
    Web(account): Web,
    Path(project_id): Path<String>,
    Json(body): Json<Rename>,
) -> ApiResult<Project> {
    let project = state
        .store
        .rename_project(&account.person_id, &project_id, &body.name)
        .await?;
    state
        .store
        .record_audit(
            &account.person_id,
            None,
            "project_renamed",
            Some(&project_id),
            serde_json::json!({"name": project.name}),
        )
        .await?;
    Ok(Json(project))
}

pub async fn transfer(
    State(state): State<AppState>,
    Web(account): Web,
    Path(project_id): Path<String>,
    Json(body): Json<Transfer>,
) -> Result<StatusCode, ApiError> {
    state
        .store
        .transfer_project(
            &account.person_id,
            account.is_admin,
            &project_id,
            &body.person_id,
        )
        .await?;
    state
        .store
        .record_audit(
            &account.person_id,
            None,
            "project_transferred",
            Some(&project_id),
            serde_json::json!({"to": body.person_id}),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The project's memories, newest first. `?limit&cursor` page it; the next cursor, when there
/// is one, is in the `x-next-cursor` header so the body stays a plain list.
pub async fn memories(
    State(state): State<AppState>,
    Agent(account): Agent,
    Path(project_id): Path<String>,
    Query(paging): Query<Paging>,
) -> Result<Response, ApiError> {
    if !state
        .store
        .is_member(&account.person_id, &project_id)
        .await?
    {
        return Err(StoreError::NotFound("project".into()).into());
    }
    let page = state
        .store
        .list(
            &account.person_id,
            &ListFilter {
                project_id: Some(project_id),
                scope: Some(Scope::Project),
                statuses: vec![Status::Active, Status::Stale, Status::Superseded],
                limit: paging.limit,
                cursor: paging.cursor,
                ..ListFilter::default()
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

pub async fn members(
    State(state): State<AppState>,
    Agent(account): Agent,
    Path(project_id): Path<String>,
) -> ApiResult<Vec<MemberRow>> {
    let members = state
        .store
        .project_members(&account.person_id, &project_id)
        .await?;
    Ok(Json(
        members
            .into_iter()
            .map(|m| MemberRow {
                person_id: m.person_id,
                email: m.email,
                role: if m.is_owner { "owner" } else { "member" },
                joined_at: m.joined_at,
                last_active_at: m.last_active_at,
            })
            .collect(),
    ))
}

pub async fn branches(
    State(state): State<AppState>,
    Agent(account): Agent,
    Path(project_id): Path<String>,
) -> ApiResult<Vec<BranchSummary>> {
    Ok(Json(
        state
            .store
            .project_branches(&account.person_id, &project_id)
            .await?,
    ))
}

pub async fn conflicts(
    State(state): State<AppState>,
    Agent(account): Agent,
    Path(project_id): Path<String>,
) -> ApiResult<Vec<Conflict>> {
    Ok(Json(
        state
            .store
            .project_conflicts(&account.person_id, &project_id)
            .await?,
    ))
}

#[derive(Deserialize)]
pub struct SessionsQuery {
    pub days: Option<i32>,
}

pub async fn sessions(
    State(state): State<AppState>,
    Agent(account): Agent,
    Path(project_id): Path<String>,
    Query(query): Query<SessionsQuery>,
) -> ApiResult<Vec<SessionReport>> {
    let days = query.days.unwrap_or(30);
    if ![7, 30, 90].contains(&days) {
        return Err(ApiError::invalid("days must be 7, 30 or 90"));
    }
    Ok(Json(
        state
            .store
            .project_sessions(&account.person_id, &project_id, days)
            .await?,
    ))
}
