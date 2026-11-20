use crate::AppState;
use crate::auth::{Agent, Web};
use crate::error::{ApiError, ApiResult};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use cairn_core::memory::{Scope, Status};
use cairn_core::project::Project;
use cairn_store::StoreError;
use cairn_store::memory::ListFilter;
use cairn_store::project::{BranchSummary, Conflict, Member};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct Listed {
    #[serde(flatten)]
    project: Project,
    role: &'static str,
}

#[derive(Deserialize)]
pub struct Init {
    #[serde(default)]
    project_id: Option<String>,
    repository: String,
    #[serde(default)]
    name: Option<String>,
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
    Ok(Json(
        projects
            .into_iter()
            .map(|project| Listed {
                role: if project.owner_id == account.person_id {
                    "owner"
                } else {
                    "member"
                },
                project,
            })
            .collect(),
    ))
}

/// `cairn init`: with a committed project id the caller joins it; otherwise one is created.
pub async fn init(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Init>,
) -> ApiResult<Initialized> {
    let (project, created) = match body.project_id.as_deref() {
        Some(id) => (
            state
                .store
                .join_project(&account.person_id, id, body.name.as_deref())
                .await?,
            false,
        ),
        None => {
            state
                .store
                .init_project(&account.person_id, &body.repository, body.name.as_deref())
                .await?
        }
    };
    Ok(Json(Initialized {
        project_id: project.project_id,
        repository: project.repository,
        name: project.name,
        created,
    }))
}

pub async fn rename(
    State(state): State<AppState>,
    Agent(account): Agent,
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
    Ok((headers, Json(page.memories)).into_response())
}

pub async fn members(
    State(state): State<AppState>,
    Agent(account): Agent,
    Path(project_id): Path<String>,
) -> ApiResult<Vec<Member>> {
    Ok(Json(
        state
            .store
            .project_members(&account.person_id, &project_id)
            .await?,
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
