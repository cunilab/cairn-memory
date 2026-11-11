//! REST compatibility transport for the Layer 1 Cairn core.

pub mod mcp;
pub mod viewer;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use cairn_core::{
    Account, AuditEvent, AuthSessionResult, CaptureResult, ContextPack, Device, DeviceRegistration,
    ExportBundle, ImportResult, LifecycleInput, LifecycleSummary, Memory, MemoryKind, MemoryStore,
    ObservationInput, Project, ProjectBranch, ProjectEntity, ProjectEntityInput,
    ProjectFreshnessEvent, ProjectMaintenanceReport, ProjectMembership, ProjectMemoryShare,
    ProjectMergeResult, ProjectWorktree, RecallFilter, RecallResult, Session, StoreError,
    SyncDiagnostics, SyncPullResult, SyncPushResult, WorkContext,
};
use cairn_storage::{PostgresError, PostgresStore};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path as FilePath;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const AUTH_ATTEMPT_LIMIT: u32 = 10;
const AUTH_ATTEMPT_WINDOW: Duration = Duration::from_secs(60);

#[derive(Default)]
struct AuthRateLimiter {
    attempts: StdMutex<HashMap<String, (Instant, u32)>>,
}

impl AuthRateLimiter {
    fn allow(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut attempts = self
            .attempts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = attempts.entry(key.to_owned()).or_insert((now, 0));
        if now.duration_since(entry.0) >= AUTH_ATTEMPT_WINDOW {
            *entry = (now, 0);
        }
        if entry.1 >= AUTH_ATTEMPT_LIMIT {
            return false;
        }
        entry.1 += 1;
        true
    }
}

#[derive(Clone)]
pub struct AppState {
    store: Arc<Mutex<MemoryStore>>,
    auth_rate_limiter: Arc<AuthRateLimiter>,
}

impl AppState {
    pub fn open(path: impl AsRef<FilePath>) -> Result<Self, StoreError> {
        Ok(Self {
            store: Arc::new(Mutex::new(MemoryStore::open(path)?)),
            auth_rate_limiter: Arc::new(AuthRateLimiter::default()),
        })
    }
}

#[derive(Clone)]
pub struct PostgresAppState {
    store: Arc<PostgresStore>,
    auth_rate_limiter: Arc<AuthRateLimiter>,
}

impl PostgresAppState {
    pub fn new(store: PostgresStore) -> Self {
        Self {
            store: Arc::new(store),
            auth_rate_limiter: Arc::new(AuthRateLimiter::default()),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct HealthResponse {
    pub ok: bool,
    pub service: &'static str,
    pub storage: &'static str,
    pub semantic_search: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct LivezResponse {
    pub ok: bool,
    pub service: &'static str,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SessionStartRequest {
    #[serde(rename = "sessionId", alias = "session_id")]
    pub session_id: String,
    #[serde(default)]
    pub context: WorkContext,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ObservationRequest {
    #[serde(rename = "eventId", alias = "event_id")]
    pub event_id: String,
    #[serde(rename = "sessionId", alias = "session_id")]
    pub session_id: String,
    pub content: String,
    pub role: String,
    pub source: String,
    #[serde(default)]
    pub importance: Option<String>,
    #[serde(default)]
    pub lifecycle: LifecycleInput,
    #[serde(default)]
    pub context: WorkContext,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SessionEndRequest {
    #[serde(rename = "sessionId", alias = "session_id")]
    pub session_id: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(rename = "sessionId", alias = "session_id", default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub context: Option<WorkContext>,
}

impl SearchRequest {
    fn recall_filter(&self) -> RecallFilter {
        RecallFilter {
            session_id: self.session_id.clone(),
            context: self.context.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ContinuityRequest {
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(rename = "sessionId", alias = "session_id", default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub context: Option<WorkContext>,
}

impl ContinuityRequest {
    fn recall_filter(&self) -> RecallFilter {
        RecallFilter {
            session_id: self.session_id.clone(),
            context: self.context.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ForgetRequest {
    pub id: Option<String>,
    #[serde(
        rename = "targetId",
        alias = "target_id",
        alias = "memoryId",
        alias = "memory_id",
        alias = "eventId",
        alias = "event_id"
    )]
    pub target_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RememberRequest {
    pub content: String,
    #[serde(default)]
    pub concepts: Vec<String>,
    #[serde(default)]
    pub context: WorkContext,
    #[serde(default)]
    pub lifecycle: LifecycleInput,
    #[serde(default)]
    pub kind: MemoryKind,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CorrectRequest {
    #[serde(
        rename = "targetId",
        alias = "target_id",
        alias = "memoryId",
        alias = "memory_id",
        alias = "eventId",
        alias = "event_id"
    )]
    pub target_id: String,
    pub content: String,
    #[serde(default)]
    pub concepts: Vec<String>,
    #[serde(default)]
    pub context: WorkContext,
    #[serde(default)]
    pub lifecycle: LifecycleInput,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProjectRegisterRequest {
    #[serde(rename = "projectId", alias = "project_id", default)]
    pub project_id: Option<String>,
    pub repository: String,
    pub name: String,
    #[serde(rename = "defaultBranch", alias = "default_branch", default)]
    pub default_branch: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PromoteRequest {
    #[serde(rename = "projectId", alias = "project_id")]
    pub project_id: String,
    #[serde(
        rename = "memoryId",
        alias = "memory_id",
        alias = "targetId",
        alias = "target_id"
    )]
    pub memory_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct VerifyProjectMemoryRequest {
    #[serde(rename = "projectId", alias = "project_id")]
    pub project_id: String,
    #[serde(rename = "memoryId", alias = "memory_id")]
    pub memory_id: String,
    pub commit: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BranchRegisterRequest {
    #[serde(rename = "projectId", alias = "project_id", default)]
    pub project_id: Option<String>,
    pub name: String,
    #[serde(rename = "baseBranch", alias = "base_branch", default)]
    pub base_branch: Option<String>,
    #[serde(rename = "baseCommit", alias = "base_commit", default)]
    pub base_commit: Option<String>,
    #[serde(rename = "headCommit", alias = "head_commit", default)]
    pub head_commit: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WorktreeRegisterRequest {
    #[serde(rename = "projectId", alias = "project_id", default)]
    pub project_id: Option<String>,
    #[serde(rename = "branchId", alias = "branch_id", default)]
    pub branch_id: Option<String>,
    pub path: String,
    #[serde(default)]
    pub commit: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BranchMergeRequest {
    #[serde(rename = "memoryId", alias = "memory_id")]
    pub memory_id: String,
    pub commit: String,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BranchCloseRequest {
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProjectEntityRequest {
    pub kind: String,
    pub key: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(rename = "sourceMemoryId", alias = "source_memory_id", default)]
    pub source_memory_id: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProjectMaintenanceRequest {
    #[serde(default, rename = "currentCommit", alias = "current_commit")]
    pub current_commit: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProjectConfirmationRequest {
    #[serde(rename = "memoryId", alias = "memory_id")]
    pub memory_id: String,
    #[serde(default)]
    pub commit: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProjectMemberRequest {
    pub email: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProjectShareRequest {
    #[serde(rename = "memoryId", alias = "memory_id")]
    pub memory_id: String,
    pub email: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProjectShareRevokeRequest {
    pub email: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ImportRequest {
    pub bundle: ExportBundle,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AccountCredentialsRequest {
    pub email: String,
    pub password: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DeviceRegisterRequest {
    #[serde(rename = "deviceId", alias = "device_id", default)]
    pub device_id: Option<String>,
    #[serde(rename = "personId", alias = "person_id")]
    pub person_id: String,
    #[serde(rename = "agentId", alias = "agent_id", default)]
    pub agent_id: Option<String>,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DeviceTargetRequest {
    #[serde(rename = "deviceId", alias = "device_id")]
    pub device_id: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SyncPushRequest {
    #[serde(flatten)]
    pub observation: ObservationRequest,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SyncPullRequest {
    #[serde(default)]
    pub cursor: u64,
}

#[derive(Debug)]
pub enum ApiError {
    Store(StoreError),
    Unauthorized(String),
    RateLimited,
}

#[derive(Debug)]
pub struct PostgresApiError(PostgresError);

impl From<PostgresError> for PostgresApiError {
    fn from(error: PostgresError) -> Self {
        Self(error)
    }
}

impl IntoResponse for PostgresApiError {
    fn into_response(self) -> Response {
        let error = self.0;
        let status = if error.is_unauthorized() {
            StatusCode::UNAUTHORIZED
        } else {
            match &error {
                PostgresError::Validation(_) | PostgresError::Core(StoreError::Validation(_)) => {
                    StatusCode::BAD_REQUEST
                }
                PostgresError::Database(_)
                | PostgresError::Core(StoreError::Io(_) | StoreError::Json(_))
                | PostgresError::Json(_) => StatusCode::INTERNAL_SERVER_ERROR,
                PostgresError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            }
        };
        (
            status,
            Json(serde_json::json!({ "error": error.to_string() })),
        )
            .into_response()
    }
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Unauthorized(message) => (StatusCode::UNAUTHORIZED, message),
            Self::RateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                "authentication rate limit exceeded".to_owned(),
            ),
            Self::Store(error) => {
                let status = match &error {
                    StoreError::Validation(_) => StatusCode::BAD_REQUEST,
                    StoreError::Io(_) | StoreError::Json(_) => StatusCode::INTERNAL_SERVER_ERROR,
                };
                (status, error.to_string())
            }
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/cairn-memory/health", get(health))
        .route("/cairn-memory/livez", get(livez))
        .route("/cairn-memory/auth/bootstrap", post(auth_bootstrap))
        .route("/cairn-memory/auth/register", post(auth_register))
        .route("/cairn-memory/auth/login", post(auth_login))
        .route("/cairn-memory/auth/logout", post(auth_logout))
        .route("/cairn-memory/auth/me", get(auth_me))
        .route("/cairn-memory/auth/audit", get(auth_audit))
        .route(
            "/cairn-memory/projects",
            get(list_projects).post(register_project),
        )
        .route("/cairn-memory/promote", post(promote))
        .route("/cairn-memory/projects/verify", post(verify_project_memory))
        .route(
            "/cairn-memory/projects/{project_id}/branches",
            get(list_project_branches).post(register_project_branch),
        )
        .route(
            "/cairn-memory/projects/{project_id}/branches/{branch_id}/merge",
            post(confirm_project_branch_merge),
        )
        .route(
            "/cairn-memory/projects/{project_id}/branches/{branch_id}/close",
            post(close_project_branch),
        )
        .route(
            "/cairn-memory/projects/{project_id}/worktrees",
            get(list_project_worktrees).post(register_project_worktree),
        )
        .route(
            "/cairn-memory/projects/{project_id}/memories",
            get(list_project_memories),
        )
        .route(
            "/cairn-memory/projects/{project_id}/members",
            post(invite_project_member),
        )
        .route(
            "/cairn-memory/projects/{project_id}/shares",
            post(share_project_memory),
        )
        .route(
            "/cairn-memory/projects/{project_id}/shares/{memory_id}/revoke",
            post(revoke_project_share),
        )
        .route(
            "/cairn-memory/projects/{project_id}/shared",
            get(list_shared_project_memories),
        )
        .route(
            "/cairn-memory/projects/{project_id}/entities",
            get(list_project_entities).post(register_project_entity),
        )
        .route(
            "/cairn-memory/projects/{project_id}/freshness",
            get(list_project_freshness),
        )
        .route(
            "/cairn-memory/projects/{project_id}/maintenance",
            post(maintain_project),
        )
        .route(
            "/cairn-memory/projects/{project_id}/confirm",
            post(confirm_project_memory),
        )
        .route("/cairn-memory/session/start", post(start_session))
        .route("/cairn-memory/observe", post(capture_observation))
        .route("/cairn-memory/session/end", post(end_session))
        .route("/cairn-memory/search", post(search))
        .route("/cairn-memory/smart-search", post(search))
        .route("/cairn-memory/why-changed", post(why_changed))
        .route("/cairn-memory/context", post(context))
        .route("/cairn-memory/recall", post(context))
        .route("/cairn-memory/resume", post(resume))
        .route("/cairn-memory/unresolved", post(unresolved))
        .route("/cairn-memory/forget", post(forget))
        .route("/cairn-memory/remember", post(remember))
        .route("/cairn-memory/correct", post(correct))
        .route("/cairn-memory/export", post(export))
        .route("/cairn-memory/import", post(import))
        .route("/cairn-memory/diagnose", get(diagnose))
        .route("/cairn-memory/devices/register", post(register_device))
        .route("/cairn-memory/devices", get(list_devices))
        .route("/cairn-memory/devices/revoke", post(revoke_device))
        .route("/cairn-memory/devices/rotate", post(rotate_device))
        .route("/cairn-memory/sync/push", post(sync_push))
        .route("/cairn-memory/sync/pull", post(sync_pull))
        .route("/cairn-memory/sync/diagnose", get(sync_diagnose))
        .route("/viewer", get(viewer::page))
        .with_state(state)
}

/// REST boundary for the server-authoritative Layer 3 PostgreSQL slice.
///
/// The file-backed `app` remains the compatibility transport for the complete
/// local Layer 1/2 API. PostgreSQL mode exposes the account, device, and sync
/// operations backed by `cairn-storage` until the remaining memory services are
/// moved behind the same repository boundary.
pub fn postgres_app(store: PostgresStore) -> Router {
    Router::new()
        .route("/cairn-memory/health", get(postgres_health))
        .route("/cairn-memory/livez", get(livez))
        .route(
            "/cairn-memory/auth/bootstrap",
            post(postgres_auth_bootstrap),
        )
        .route("/cairn-memory/auth/register", post(postgres_auth_register))
        .route("/cairn-memory/auth/login", post(postgres_auth_login))
        .route("/cairn-memory/auth/logout", post(postgres_auth_logout))
        .route("/cairn-memory/auth/me", get(postgres_auth_me))
        .route("/cairn-memory/auth/audit", get(postgres_auth_audit))
        .route(
            "/cairn-memory/projects",
            get(postgres_list_projects).post(postgres_register_project),
        )
        .route("/cairn-memory/promote", post(postgres_promote))
        .route(
            "/cairn-memory/projects/verify",
            post(postgres_verify_project_memory),
        )
        .route(
            "/cairn-memory/projects/{project_id}/branches",
            get(postgres_list_project_branches).post(postgres_register_project_branch),
        )
        .route(
            "/cairn-memory/projects/{project_id}/branches/{branch_id}/merge",
            post(postgres_confirm_project_branch_merge),
        )
        .route(
            "/cairn-memory/projects/{project_id}/branches/{branch_id}/close",
            post(postgres_close_project_branch),
        )
        .route(
            "/cairn-memory/projects/{project_id}/worktrees",
            get(postgres_list_project_worktrees).post(postgres_register_project_worktree),
        )
        .route(
            "/cairn-memory/projects/{project_id}/memories",
            get(postgres_list_project_memories),
        )
        .route(
            "/cairn-memory/projects/{project_id}/members",
            post(postgres_invite_project_member),
        )
        .route(
            "/cairn-memory/projects/{project_id}/shares",
            post(postgres_share_project_memory),
        )
        .route(
            "/cairn-memory/projects/{project_id}/shares/{memory_id}/revoke",
            post(postgres_revoke_project_share),
        )
        .route(
            "/cairn-memory/projects/{project_id}/shared",
            get(postgres_list_shared_project_memories),
        )
        .route(
            "/cairn-memory/projects/{project_id}/entities",
            get(postgres_list_project_entities).post(postgres_register_project_entity),
        )
        .route(
            "/cairn-memory/projects/{project_id}/freshness",
            get(postgres_list_project_freshness),
        )
        .route(
            "/cairn-memory/projects/{project_id}/maintenance",
            post(postgres_maintain_project),
        )
        .route(
            "/cairn-memory/projects/{project_id}/confirm",
            post(postgres_confirm_project_memory),
        )
        .route("/cairn-memory/session/start", post(postgres_start_session))
        .route("/cairn-memory/session/end", post(postgres_end_session))
        .route("/cairn-memory/search", post(postgres_search))
        .route("/cairn-memory/smart-search", post(postgres_search))
        .route("/cairn-memory/context", post(postgres_context))
        .route("/cairn-memory/recall", post(postgres_context))
        .route("/cairn-memory/resume", post(postgres_resume))
        .route("/cairn-memory/unresolved", post(postgres_unresolved))
        .route("/cairn-memory/why-changed", post(postgres_why_changed))
        .route("/cairn-memory/remember", post(postgres_remember))
        .route("/cairn-memory/correct", post(postgres_correct))
        .route("/cairn-memory/export", post(postgres_export))
        .route("/cairn-memory/import", post(postgres_import))
        .route("/cairn-memory/diagnose", get(postgres_diagnose))
        .route("/cairn-memory/forget", post(postgres_forget))
        .route(
            "/cairn-memory/devices/register",
            post(postgres_register_device),
        )
        .route("/cairn-memory/devices", get(postgres_list_devices))
        .route("/cairn-memory/devices/revoke", post(postgres_revoke_device))
        .route("/cairn-memory/devices/rotate", post(postgres_rotate_device))
        .route("/cairn-memory/sync/push", post(postgres_sync_push))
        .route("/cairn-memory/sync/pull", post(postgres_sync_pull))
        .route("/cairn-memory/sync/diagnose", get(postgres_sync_diagnose))
        .route("/viewer", get(viewer::page))
        .with_state(PostgresAppState::new(store))
}

async fn postgres_health() -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        service: "cairn-server",
        storage: "postgresql-layer-3",
        semantic_search: "disabled",
    })
}

async fn postgres_auth_bootstrap(
    State(state): State<PostgresAppState>,
    Json(request): Json<AccountCredentialsRequest>,
) -> Result<Json<AuthSessionResult>, PostgresApiError> {
    if !state
        .auth_rate_limiter
        .allow(&auth_rate_key("bootstrap", &request.email))
    {
        return Err(PostgresApiError(PostgresError::RateLimited));
    }
    Ok(Json(
        state
            .store
            .bootstrap_account(&request.email, &request.password)
            .await?,
    ))
}

async fn postgres_auth_register(
    State(state): State<PostgresAppState>,
    Json(request): Json<AccountCredentialsRequest>,
) -> Result<Json<AuthSessionResult>, PostgresApiError> {
    if !state
        .auth_rate_limiter
        .allow(&auth_rate_key("register", &request.email))
    {
        return Err(PostgresApiError(PostgresError::RateLimited));
    }
    Ok(Json(
        state
            .store
            .register_account(&request.email, &request.password)
            .await?,
    ))
}

async fn postgres_auth_login(
    State(state): State<PostgresAppState>,
    Json(request): Json<AccountCredentialsRequest>,
) -> Result<Json<AuthSessionResult>, PostgresApiError> {
    if !state
        .auth_rate_limiter
        .allow(&auth_rate_key("login", &request.email))
    {
        return Err(PostgresApiError(PostgresError::RateLimited));
    }
    Ok(Json(
        state.store.login(&request.email, &request.password).await?,
    ))
}

async fn postgres_auth_logout(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
) -> Result<StatusCode, PostgresApiError> {
    state
        .store
        .logout_account(postgres_token(&headers)?)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn postgres_auth_me(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
) -> Result<Json<Account>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .authenticate_account(postgres_token(&headers)?)
            .await?,
    ))
}

async fn postgres_auth_audit(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<AuditEvent>>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .audit_events(postgres_token(&headers)?, 100)
            .await?,
    ))
}

async fn postgres_register_project(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<ProjectRegisterRequest>,
) -> Result<Json<Project>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .register_project(
                postgres_token(&headers)?,
                &request.repository,
                &request.name,
                request.default_branch,
            )
            .await?,
    ))
}

async fn postgres_list_projects(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Project>>, PostgresApiError> {
    Ok(Json(state.store.projects(postgres_token(&headers)?).await?))
}

async fn postgres_promote(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<PromoteRequest>,
) -> Result<Json<Memory>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .promote_memory(
                postgres_token(&headers)?,
                &request.project_id,
                &request.memory_id,
                &request.reason,
            )
            .await?,
    ))
}

async fn postgres_verify_project_memory(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<VerifyProjectMemoryRequest>,
) -> Result<Json<Memory>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .verify_project_memory(
                postgres_token(&headers)?,
                &request.project_id,
                &request.memory_id,
                &request.commit,
            )
            .await?,
    ))
}

async fn postgres_confirm_project_branch_merge(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path((project_id, branch_id)): Path<(String, String)>,
    Json(request): Json<BranchMergeRequest>,
) -> Result<Json<ProjectMergeResult>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .confirm_branch_merge(
                postgres_token(&headers)?,
                &project_id,
                &branch_id,
                &request.memory_id,
                &request.commit,
                &request.reason,
            )
            .await?,
    ))
}

async fn postgres_close_project_branch(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path((project_id, branch_id)): Path<(String, String)>,
    Json(request): Json<BranchCloseRequest>,
) -> Result<Json<ProjectBranch>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .close_branch(
                postgres_token(&headers)?,
                &project_id,
                &branch_id,
                &request.reason,
            )
            .await?,
    ))
}

async fn postgres_register_project_branch(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<BranchRegisterRequest>,
) -> Result<Json<ProjectBranch>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .register_branch(
                postgres_token(&headers)?,
                &project_id,
                &request.name,
                request.base_branch,
                request.base_commit,
                request.head_commit,
            )
            .await?,
    ))
}

async fn postgres_list_project_branches(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<ProjectBranch>>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .branches(postgres_token(&headers)?, &project_id)
            .await?,
    ))
}

async fn postgres_register_project_worktree(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<WorktreeRegisterRequest>,
) -> Result<Json<ProjectWorktree>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .register_worktree(
                postgres_token(&headers)?,
                &project_id,
                request.branch_id,
                &request.path,
                request.commit,
            )
            .await?,
    ))
}

async fn postgres_list_project_worktrees(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<ProjectWorktree>>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .worktrees(postgres_token(&headers)?, &project_id)
            .await?,
    ))
}

async fn postgres_list_project_memories(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<Memory>>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .project_memories(postgres_token(&headers)?, &project_id)
            .await?,
    ))
}

async fn postgres_invite_project_member(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<ProjectMemberRequest>,
) -> Result<Json<ProjectMembership>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .invite_project_member(postgres_token(&headers)?, &project_id, &request.email)
            .await?,
    ))
}

async fn postgres_share_project_memory(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<ProjectShareRequest>,
) -> Result<Json<ProjectMemoryShare>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .share_project_memory(
                postgres_token(&headers)?,
                &project_id,
                &request.memory_id,
                &request.email,
            )
            .await?,
    ))
}

async fn postgres_revoke_project_share(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path((project_id, memory_id)): Path<(String, String)>,
    Json(request): Json<ProjectShareRevokeRequest>,
) -> Result<Json<ProjectMemoryShare>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .revoke_project_share(
                postgres_token(&headers)?,
                &project_id,
                &memory_id,
                &request.email,
            )
            .await?,
    ))
}

async fn postgres_list_shared_project_memories(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<Memory>>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .shared_project_memories(postgres_token(&headers)?, &project_id)
            .await?,
    ))
}

async fn postgres_register_project_entity(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<ProjectEntityRequest>,
) -> Result<Json<ProjectEntity>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .register_project_entity(
                postgres_token(&headers)?,
                &project_id,
                ProjectEntityInput {
                    kind: request.kind,
                    key: request.key,
                    content: request.content,
                    source_memory_id: request.source_memory_id,
                    commit: request.commit,
                },
            )
            .await?,
    ))
}

async fn postgres_list_project_entities(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<ProjectEntity>>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .project_entities(postgres_token(&headers)?, &project_id)
            .await?,
    ))
}

async fn postgres_list_project_freshness(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<ProjectFreshnessEvent>>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .freshness_history(postgres_token(&headers)?, &project_id)
            .await?,
    ))
}

async fn postgres_maintain_project(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<ProjectMaintenanceRequest>,
) -> Result<Json<ProjectMaintenanceReport>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .project_maintenance(
                postgres_token(&headers)?,
                &project_id,
                request.current_commit.as_deref(),
            )
            .await?,
    ))
}

async fn postgres_confirm_project_memory(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<ProjectConfirmationRequest>,
) -> Result<Json<Memory>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .confirm_memory(
                postgres_token(&headers)?,
                &project_id,
                &request.memory_id,
                request.commit.as_deref(),
                &request.reason,
            )
            .await?,
    ))
}

async fn postgres_start_session(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<SessionStartRequest>,
) -> Result<Json<Session>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .start_session(
                postgres_token(&headers)?,
                &request.session_id,
                request.context,
            )
            .await?,
    ))
}

async fn postgres_end_session(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<SessionEndRequest>,
) -> Result<Json<Memory>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .end_session(postgres_token(&headers)?, &request.session_id)
            .await?,
    ))
}

async fn postgres_search(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<SearchRequest>,
) -> Result<Json<Vec<RecallResult>>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .search(
                postgres_token(&headers)?,
                &request.query,
                request.limit,
                &request.recall_filter(),
            )
            .await?,
    ))
}

async fn postgres_context(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<SearchRequest>,
) -> Result<Json<ContextPack>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .context(
                postgres_token(&headers)?,
                &request.query,
                request.limit,
                &request.recall_filter(),
            )
            .await?,
    ))
}

async fn postgres_resume(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<ContinuityRequest>,
) -> Result<Json<ContextPack>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .resume_context(
                postgres_token(&headers)?,
                request.limit,
                &request.recall_filter(),
            )
            .await?,
    ))
}

async fn postgres_unresolved(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<ContinuityRequest>,
) -> Result<Json<ContextPack>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .unresolved_context(
                postgres_token(&headers)?,
                request.limit,
                &request.recall_filter(),
            )
            .await?,
    ))
}

async fn postgres_why_changed(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<SearchRequest>,
) -> Result<Json<ContextPack>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .why_changed_context(
                postgres_token(&headers)?,
                &request.query,
                request.limit,
                &request.recall_filter(),
            )
            .await?,
    ))
}

async fn postgres_remember(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<RememberRequest>,
) -> Result<Json<Memory>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .remember_with_kind(
                postgres_token(&headers)?,
                &request.content,
                request.concepts,
                request.context,
                request.lifecycle,
                request.kind,
            )
            .await?,
    ))
}

async fn postgres_correct(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<CorrectRequest>,
) -> Result<Json<Memory>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .correct(
                postgres_token(&headers)?,
                &request.target_id,
                &request.content,
                request.concepts,
                request.context,
                request.lifecycle,
            )
            .await?,
    ))
}

async fn postgres_export(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
) -> Result<Json<ExportBundle>, PostgresApiError> {
    Ok(Json(
        state.store.export_bundle(postgres_token(&headers)?).await?,
    ))
}

async fn postgres_import(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<ImportRequest>,
) -> Result<Json<ImportResult>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .import_bundle(postgres_token(&headers)?, request.bundle)
            .await?,
    ))
}

async fn postgres_diagnose(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
) -> Result<Json<LifecycleSummary>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .lifecycle_summary(postgres_token(&headers)?)
            .await?,
    ))
}

async fn postgres_forget(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<ForgetRequest>,
) -> Result<StatusCode, PostgresApiError> {
    let id = request
        .id
        .or(request.target_id)
        .ok_or_else(|| PostgresApiError(PostgresError::Validation("id is required".to_owned())))?;
    state.store.forget(postgres_token(&headers)?, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn postgres_register_device(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<DeviceRegisterRequest>,
) -> Result<Json<DeviceRegistration>, PostgresApiError> {
    let device_id = request.device_id.unwrap_or_else(generated_device_id);
    Ok(Json(
        state
            .store
            .register_device(
                postgres_token(&headers)?,
                &device_id,
                &request.person_id,
                request.agent_id.as_deref(),
                &request.name,
            )
            .await?,
    ))
}

async fn postgres_list_devices(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Device>>, PostgresApiError> {
    Ok(Json(
        state.store.list_devices(postgres_token(&headers)?).await?,
    ))
}

async fn postgres_revoke_device(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<DeviceTargetRequest>,
) -> Result<Json<Device>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .revoke_device(postgres_token(&headers)?, &request.device_id)
            .await?,
    ))
}

async fn postgres_rotate_device(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<DeviceTargetRequest>,
) -> Result<Json<DeviceRegistration>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .rotate_device(postgres_token(&headers)?, &request.device_id)
            .await?,
    ))
}

async fn postgres_sync_push(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<SyncPushRequest>,
) -> Result<Json<SyncPushResult>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .sync_push(
                postgres_token(&headers)?,
                observation_input(request.observation),
            )
            .await?,
    ))
}

async fn postgres_sync_pull(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<SyncPullRequest>,
) -> Result<Json<SyncPullResult>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .sync_pull(postgres_token(&headers)?, request.cursor)
            .await?,
    ))
}

async fn postgres_sync_diagnose(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
) -> Result<Json<SyncDiagnostics>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .sync_diagnostics(postgres_token(&headers)?)
            .await?,
    ))
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        service: "cairn-server",
        storage: "file-backed-layer-3-prototype",
        semantic_search: "disabled",
    })
}

async fn livez() -> Json<LivezResponse> {
    Json(LivezResponse {
        ok: true,
        service: "cairn-server",
    })
}

async fn auth_bootstrap(
    State(state): State<AppState>,
    Json(request): Json<AccountCredentialsRequest>,
) -> Result<Json<AuthSessionResult>, ApiError> {
    if !state
        .auth_rate_limiter
        .allow(&auth_rate_key("bootstrap", &request.email))
    {
        return Err(ApiError::RateLimited);
    }
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .bootstrap_account(&request.email, &request.password)
            .map_err(account_api_error)?,
    ))
}

async fn auth_register(
    State(state): State<AppState>,
    Json(request): Json<AccountCredentialsRequest>,
) -> Result<Json<AuthSessionResult>, ApiError> {
    if !state
        .auth_rate_limiter
        .allow(&auth_rate_key("register", &request.email))
    {
        return Err(ApiError::RateLimited);
    }
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .register_account(&request.email, &request.password)
            .map_err(account_api_error)?,
    ))
}

async fn auth_login(
    State(state): State<AppState>,
    Json(request): Json<AccountCredentialsRequest>,
) -> Result<Json<AuthSessionResult>, ApiError> {
    if !state
        .auth_rate_limiter
        .allow(&auth_rate_key("login", &request.email))
    {
        return Err(ApiError::RateLimited);
    }
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .login(&request.email, &request.password)
            .map_err(account_api_error)?,
    ))
}

async fn auth_logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let token = auth_token(&headers)?;
    let mut store = state.store.lock().await;
    store.logout_account(token).map_err(account_api_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn auth_me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Account>, ApiError> {
    let token = auth_token(&headers)?;
    let store = state.store.lock().await;
    Ok(Json(
        store
            .authenticate_account(token)
            .map_err(account_api_error)?,
    ))
}

async fn auth_audit(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<AuditEvent>>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(
        store
            .audit_events(auth_token(&headers)?, 100)
            .map_err(account_api_error)?,
    ))
}

async fn register_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ProjectRegisterRequest>,
) -> Result<Json<Project>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .register_project(
                auth_token(&headers)?,
                &request.repository,
                &request.name,
                request.default_branch,
            )
            .map_err(account_api_error)?,
    ))
}

async fn list_projects(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Project>>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(
        store
            .projects(auth_token(&headers)?)
            .map_err(account_api_error)?,
    ))
}

async fn promote(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<PromoteRequest>,
) -> Result<Json<Memory>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .promote_memory(
                auth_token(&headers)?,
                &request.project_id,
                &request.memory_id,
                &request.reason,
            )
            .map_err(account_api_error)?,
    ))
}

async fn verify_project_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<VerifyProjectMemoryRequest>,
) -> Result<Json<Memory>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .verify_project_memory(
                auth_token(&headers)?,
                &request.project_id,
                &request.memory_id,
                &request.commit,
            )
            .map_err(account_api_error)?,
    ))
}

async fn confirm_project_branch_merge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project_id, branch_id)): Path<(String, String)>,
    Json(request): Json<BranchMergeRequest>,
) -> Result<Json<ProjectMergeResult>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .confirm_branch_merge(
                auth_token(&headers)?,
                &project_id,
                &branch_id,
                &request.memory_id,
                &request.commit,
                &request.reason,
            )
            .map_err(account_api_error)?,
    ))
}

async fn close_project_branch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project_id, branch_id)): Path<(String, String)>,
    Json(request): Json<BranchCloseRequest>,
) -> Result<Json<ProjectBranch>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .close_branch(
                auth_token(&headers)?,
                &project_id,
                &branch_id,
                &request.reason,
            )
            .map_err(account_api_error)?,
    ))
}

async fn register_project_branch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<BranchRegisterRequest>,
) -> Result<Json<ProjectBranch>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .register_branch(
                auth_token(&headers)?,
                &project_id,
                &request.name,
                request.base_branch,
                request.base_commit,
                request.head_commit,
            )
            .map_err(account_api_error)?,
    ))
}

async fn list_project_branches(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<ProjectBranch>>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(
        store
            .branches(auth_token(&headers)?, &project_id)
            .map_err(account_api_error)?,
    ))
}

async fn register_project_worktree(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<WorktreeRegisterRequest>,
) -> Result<Json<ProjectWorktree>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .register_worktree(
                auth_token(&headers)?,
                &project_id,
                request.branch_id,
                &request.path,
                request.commit,
            )
            .map_err(account_api_error)?,
    ))
}

async fn list_project_worktrees(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<ProjectWorktree>>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(
        store
            .worktrees(auth_token(&headers)?, &project_id)
            .map_err(account_api_error)?,
    ))
}

async fn list_project_memories(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<Memory>>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(
        store
            .project_memories(auth_token(&headers)?, &project_id)
            .map_err(account_api_error)?,
    ))
}

async fn invite_project_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<ProjectMemberRequest>,
) -> Result<Json<ProjectMembership>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .invite_project_member(auth_token(&headers)?, &project_id, &request.email)
            .map_err(account_api_error)?,
    ))
}

async fn share_project_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<ProjectShareRequest>,
) -> Result<Json<ProjectMemoryShare>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .share_project_memory(
                auth_token(&headers)?,
                &project_id,
                &request.memory_id,
                &request.email,
            )
            .map_err(account_api_error)?,
    ))
}

async fn revoke_project_share(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project_id, memory_id)): Path<(String, String)>,
    Json(request): Json<ProjectShareRevokeRequest>,
) -> Result<Json<ProjectMemoryShare>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .revoke_project_share(
                auth_token(&headers)?,
                &project_id,
                &memory_id,
                &request.email,
            )
            .map_err(account_api_error)?,
    ))
}

async fn list_shared_project_memories(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<Memory>>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(
        store
            .shared_project_memories(auth_token(&headers)?, &project_id)
            .map_err(account_api_error)?,
    ))
}

async fn register_project_entity(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<ProjectEntityRequest>,
) -> Result<Json<ProjectEntity>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .register_project_entity(
                auth_token(&headers)?,
                &project_id,
                ProjectEntityInput {
                    kind: request.kind,
                    key: request.key,
                    content: request.content,
                    source_memory_id: request.source_memory_id,
                    commit: request.commit,
                },
            )
            .map_err(account_api_error)?,
    ))
}

async fn list_project_entities(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<ProjectEntity>>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(
        store
            .project_entities(auth_token(&headers)?, &project_id)
            .map_err(account_api_error)?,
    ))
}

async fn list_project_freshness(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<ProjectFreshnessEvent>>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(
        store
            .freshness_history(auth_token(&headers)?, &project_id)
            .map_err(account_api_error)?,
    ))
}

async fn maintain_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<ProjectMaintenanceRequest>,
) -> Result<Json<ProjectMaintenanceReport>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(
        store
            .project_maintenance(
                auth_token(&headers)?,
                &project_id,
                request.current_commit.as_deref(),
            )
            .map_err(account_api_error)?,
    ))
}

async fn confirm_project_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<ProjectConfirmationRequest>,
) -> Result<Json<Memory>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .confirm_memory(
                auth_token(&headers)?,
                &project_id,
                &request.memory_id,
                request.commit.as_deref(),
                &request.reason,
            )
            .map_err(account_api_error)?,
    ))
}

async fn start_session(
    State(state): State<AppState>,
    Json(request): Json<SessionStartRequest>,
) -> Result<Json<Session>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(
        store.start_session(request.session_id, request.context)?,
    ))
}

async fn capture_observation(
    State(state): State<AppState>,
    Json(request): Json<ObservationRequest>,
) -> Result<Json<CaptureResult>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(store.capture_observation(observation_input(request))?))
}

async fn end_session(
    State(state): State<AppState>,
    Json(request): Json<SessionEndRequest>,
) -> Result<Json<Memory>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(store.end_session(&request.session_id)?))
}

async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SearchRequest>,
) -> Result<Json<Vec<RecallResult>>, ApiError> {
    let store = state.store.lock().await;
    if let Some(token) = optional_auth_token(&headers) {
        return Ok(Json(
            store
                .search_for_account(
                    token,
                    &request.query,
                    request.limit,
                    &request.recall_filter(),
                )
                .map_err(account_api_error)?,
        ));
    }
    Ok(Json(store.search_with_filter(
        &request.query,
        request.limit,
        &request.recall_filter(),
    )?))
}

async fn why_changed(
    State(state): State<AppState>,
    Json(request): Json<SearchRequest>,
) -> Result<Json<ContextPack>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(store.why_changed_context(
        &request.query,
        request.limit,
        &request.recall_filter(),
    )?))
}

async fn context(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SearchRequest>,
) -> Result<Json<ContextPack>, ApiError> {
    let store = state.store.lock().await;
    if let Some(token) = optional_auth_token(&headers) {
        return Ok(Json(
            store
                .context_for_account(
                    token,
                    &request.query,
                    request.limit,
                    &request.recall_filter(),
                )
                .map_err(account_api_error)?,
        ));
    }
    Ok(Json(store.context_with_filter(
        &request.query,
        request.limit,
        &request.recall_filter(),
    )?))
}

async fn resume(
    State(state): State<AppState>,
    Json(request): Json<ContinuityRequest>,
) -> Result<Json<ContextPack>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(
        store.resume_context(request.limit, &request.recall_filter())?,
    ))
}

async fn unresolved(
    State(state): State<AppState>,
    Json(request): Json<ContinuityRequest>,
) -> Result<Json<ContextPack>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(store.unresolved_context(
        request.limit,
        &request.recall_filter(),
    )?))
}

async fn forget(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ForgetRequest>,
) -> Result<StatusCode, ApiError> {
    let id = request
        .id
        .or(request.target_id)
        .ok_or_else(|| ApiError::from(StoreError::Validation("id is required".to_owned())))?;
    let mut store = state.store.lock().await;
    if let Some(token) = optional_auth_token(&headers) {
        store
            .forget_for_account(token, &id)
            .map_err(account_api_error)?;
    } else {
        store.forget(&id)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn remember(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RememberRequest>,
) -> Result<Json<Memory>, ApiError> {
    let mut store = state.store.lock().await;
    if let Some(token) = optional_auth_token(&headers) {
        return Ok(Json(
            store
                .remember_for_account(
                    token,
                    &request.content,
                    request.concepts,
                    request.context,
                    request.lifecycle,
                    request.kind,
                )
                .map_err(account_api_error)?,
        ));
    }
    Ok(Json(store.remember_with_kind(
        &request.content,
        request.concepts,
        request.context,
        request.lifecycle,
        request.kind,
    )?))
}

async fn correct(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CorrectRequest>,
) -> Result<Json<Memory>, ApiError> {
    let mut store = state.store.lock().await;
    if let Some(token) = optional_auth_token(&headers) {
        return Ok(Json(
            store
                .correct_for_account(
                    token,
                    &request.target_id,
                    &request.content,
                    request.concepts,
                    request.context,
                    request.lifecycle,
                )
                .map_err(account_api_error)?,
        ));
    }
    Ok(Json(store.correct(
        &request.target_id,
        &request.content,
        request.concepts,
        request.context,
        request.lifecycle,
    )?))
}

async fn export(State(state): State<AppState>) -> Result<Json<ExportBundle>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(store.export()))
}

async fn import(
    State(state): State<AppState>,
    Json(request): Json<ImportRequest>,
) -> Result<Json<ImportResult>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(store.import_bundle(request.bundle)?))
}

async fn diagnose(State(state): State<AppState>) -> Result<Json<LifecycleSummary>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(store.lifecycle_summary()))
}

async fn register_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<DeviceRegisterRequest>,
) -> Result<Json<DeviceRegistration>, ApiError> {
    let device_id = request.device_id.unwrap_or_else(|| {
        format!(
            "device-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )
    });
    let mut store = state.store.lock().await;
    let account = store
        .authenticate_account(auth_token(&headers)?)
        .map_err(account_api_error)?;
    if account.person_id != request.person_id {
        return Err(ApiError::Unauthorized(
            "account does not own the requested person".to_owned(),
        ));
    }
    Ok(Json(store.register_device(
        device_id,
        request.person_id,
        request.agent_id,
        request.name,
    )?))
}

async fn list_devices(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Device>>, ApiError> {
    let token = auth_token(&headers)?;
    let mut store = state.store.lock().await;
    let caller = store.authenticate_device(token).map_err(device_api_error)?;
    let devices = store
        .devices()
        .filter(|device| device.person_id == caller.person_id)
        .cloned()
        .collect();
    Ok(Json(devices))
}

async fn revoke_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<DeviceTargetRequest>,
) -> Result<Json<Device>, ApiError> {
    let token = auth_token(&headers)?;
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .revoke_device(token, &request.device_id)
            .map_err(device_api_error)?,
    ))
}

async fn rotate_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<DeviceTargetRequest>,
) -> Result<Json<DeviceRegistration>, ApiError> {
    let token = auth_token(&headers)?;
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .rotate_device_token(token, &request.device_id)
            .map_err(device_api_error)?,
    ))
}

async fn sync_push(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SyncPushRequest>,
) -> Result<Json<SyncPushResult>, ApiError> {
    let token = auth_token(&headers)?;
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .sync_push(token, observation_input(request.observation))
            .map_err(device_api_error)?,
    ))
}

async fn sync_pull(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SyncPullRequest>,
) -> Result<Json<SyncPullResult>, ApiError> {
    let token = auth_token(&headers)?;
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .sync_pull(token, request.cursor)
            .map_err(device_api_error)?,
    ))
}

async fn sync_diagnose(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SyncDiagnostics>, ApiError> {
    let token = auth_token(&headers)?;
    let mut store = state.store.lock().await;
    Ok(Json(
        store.sync_diagnostics(token).map_err(device_api_error)?,
    ))
}

fn observation_input(request: ObservationRequest) -> ObservationInput {
    ObservationInput {
        event_id: request.event_id,
        session_id: request.session_id,
        content: request.content,
        role: request.role,
        source: request.source,
        importance: request.importance,
        lifecycle: request.lifecycle,
        context: request.context,
    }
}

fn optional_auth_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.trim().is_empty())
}

fn auth_token(headers: &HeaderMap) -> Result<&str, ApiError> {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ApiError::Unauthorized("Bearer token is required".to_owned()))
}

fn postgres_token(headers: &HeaderMap) -> Result<&str, PostgresApiError> {
    auth_token(headers).map_err(|_| {
        PostgresApiError(PostgresError::Validation(
            "authentication failed".to_owned(),
        ))
    })
}

fn generated_device_id() -> String {
    format!(
        "device-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}

fn account_api_error(error: StoreError) -> ApiError {
    match &error {
        StoreError::Validation(message)
            if message.contains("authentication failed")
                || message.contains("expired or revoked")
                || message.contains("account is disabled")
                || message.contains("invalid account credentials")
                || message.contains("does not own") =>
        {
            ApiError::Unauthorized(message.clone())
        }
        _ => ApiError::from(error),
    }
}

fn device_api_error(error: StoreError) -> ApiError {
    match &error {
        StoreError::Validation(message)
            if message.contains("authentication failed")
                || message.contains("device is revoked") =>
        {
            ApiError::Unauthorized(message.clone())
        }
        _ => ApiError::from(error),
    }
}

fn default_limit() -> usize {
    5
}

fn auth_rate_key(operation: &str, email: &str) -> String {
    format!("{operation}:{}", email.trim().to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::Value;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tower::util::ServiceExt;

    fn temp_store_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "cairn-server-{name}-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    async fn json_request(
        router: Router,
        method: &str,
        path: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let response = router
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "response body was not JSON (status {status}): {error}; body={}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, value)
    }

    async fn json_request_auth(
        router: Router,
        method: &str,
        path: &str,
        body: Value,
        token: &str,
    ) -> (StatusCode, Value) {
        let response = router
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "response body was not JSON (status {status}): {error}; body={}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, value)
    }

    #[tokio::test]
    async fn health_and_livez_are_available() {
        let path = temp_store_path("health");
        let state = AppState::open(&path).unwrap();
        let router = app(state);

        let (status, health) =
            json_request(router.clone(), "GET", "/cairn-memory/health", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(health["ok"], true);

        let (status, livez) = json_request(router, "GET", "/cairn-memory/livez", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(livez["ok"], true);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn self_hosted_account_auth_bootstraps_logs_in_and_revokes_sessions() {
        let path = temp_store_path("account-auth");
        let router = app(AppState::open(&path).unwrap());
        let password = "correct horse battery staple";
        let (status, bootstrap) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({
                "email": "Owner@Example.test",
                "password": password
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(bootstrap["account"]["email"], "owner@example.test");
        let first_token = bootstrap["token"].as_str().unwrap().to_owned();
        let persisted = std::fs::read_to_string(&path).unwrap();
        assert!(!persisted.contains(password));
        assert!(!persisted.contains(&first_token));

        let (status, me) = json_request_auth(
            router.clone(),
            "GET",
            "/cairn-memory/auth/me",
            Value::Null,
            &first_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(me["person_id"], bootstrap["account"]["person_id"]);

        let (status, _) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/login",
            serde_json::json!({
                "email": "owner@example.test",
                "password": "wrong password"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, login) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/login",
            serde_json::json!({
                "email": "owner@example.test",
                "password": password
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let second_token = login["token"].as_str().unwrap().to_owned();
        assert_ne!(second_token, first_token);

        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/auth/logout",
            Value::Null,
            &first_token,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = json_request_auth(
            router,
            "GET",
            "/cairn-memory/auth/me",
            Value::Null,
            &first_token,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn account_login_attempts_are_rate_limited() {
        let path = temp_store_path("auth-rate-limit");
        let router = app(AppState::open(&path).unwrap());
        let (_, _) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({
                "email": "rate-limit@example.test",
                "password": "correct horse battery staple"
            }),
        )
        .await;
        for _ in 0..10 {
            let (status, _) = json_request(
                router.clone(),
                "POST",
                "/cairn-memory/auth/login",
                serde_json::json!({
                    "email": "rate-limit@example.test",
                    "password": "wrong password"
                }),
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
        let (status, _) = json_request(
            router,
            "POST",
            "/cairn-memory/auth/login",
            serde_json::json!({
                "email": "rate-limit@example.test",
                "password": "wrong password"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn device_sync_registers_replays_once_rotates_and_revokes() {
        let path = temp_store_path("device-sync");
        let router = app(AppState::open(&path).unwrap());
        let (_, bootstrap) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({
                "email": "sync-owner@example.test",
                "password": "correct horse battery staple"
            }),
        )
        .await;
        let account_token = bootstrap["token"].as_str().unwrap();
        let person_id = bootstrap["account"]["person_id"].as_str().unwrap();
        let (status, first) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/devices/register",
            serde_json::json!({
                "deviceId": "device-a",
                "personId": person_id,
                "agentId": "agent-a",
                "name": "MacBook"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let first_token = first["token"].as_str().unwrap().to_owned();
        let (status, second) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/devices/register",
            serde_json::json!({
                "deviceId": "device-b",
                "personId": person_id,
                "name": "Laptop"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let second_token = second["token"].as_str().unwrap().to_owned();

        let payload = serde_json::json!({
            "eventId": "sync-event-1",
            "sessionId": "sync-session",
            "content": "Cross-device memory was captured.",
            "role": "agent",
            "source": "sync",
            "context": {"repository": "cairn-demo"}
        });
        let (status, pushed) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/sync/push",
            payload.clone(),
            &first_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(pushed["duplicate"], false);

        let (status, duplicate) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/sync/push",
            payload,
            &first_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(duplicate["duplicate"], true);
        assert_eq!(duplicate["cursor"], pushed["cursor"]);

        let (status, pulled) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/sync/pull",
            serde_json::json!({"cursor": 0}),
            &second_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(pulled["events"].as_array().unwrap().len(), 1);
        let cursor = pulled["cursor"].as_u64().unwrap();
        let (status, resumed) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/sync/pull",
            serde_json::json!({"cursor": cursor}),
            &second_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(resumed["events"].as_array().unwrap().is_empty());

        let (status, rotated) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/devices/rotate",
            serde_json::json!({"deviceId": "device-a"}),
            &first_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let rotated_token = rotated["token"].as_str().unwrap();
        assert_ne!(rotated_token, first_token);
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/sync/pull",
            serde_json::json!({"cursor": 0}),
            &first_token,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, revoked) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/devices/revoke",
            serde_json::json!({"deviceId": "device-b"}),
            rotated_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(revoked["status"], "revoked");
        let (status, _) = json_request_auth(
            router,
            "POST",
            "/cairn-memory/sync/pull",
            serde_json::json!({"cursor": cursor}),
            &second_token,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn device_registration_requires_account_ownership() {
        let path = temp_store_path("device-registration-auth");
        let router = app(AppState::open(&path).unwrap());
        let (_, bootstrap) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({
                "email": "device-owner@example.test",
                "password": "correct horse battery staple"
            }),
        )
        .await;
        let account_token = bootstrap["token"].as_str().unwrap();
        let person_id = bootstrap["account"]["person_id"].as_str().unwrap();

        let (status, _) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/devices/register",
            serde_json::json!({
                "deviceId": "device-no-auth",
                "personId": person_id,
                "name": "No Auth"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/devices/register",
            serde_json::json!({
                "deviceId": "device-wrong-person",
                "personId": "person-other",
                "name": "Wrong Person"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, registration) = json_request_auth(
            router,
            "POST",
            "/cairn-memory/devices/register",
            serde_json::json!({
                "deviceId": "device-authorized",
                "personId": person_id,
                "name": "Authorized"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(registration["device"]["person_id"], person_id);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn project_promotion_preserves_evidence_and_blocks_secrets() {
        let path = temp_store_path("project-promotion");
        let router = app(AppState::open(&path).unwrap());
        let (_, bootstrap) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({
                "email": "project-owner@example.test",
                "password": "correct horse battery staple"
            }),
        )
        .await;
        let account_token = bootstrap["token"].as_str().unwrap();
        let (status, project) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/projects",
            serde_json::json!({
                "repository": "cairn-demo",
                "name": "Cairn Demo",
                "defaultBranch": "main"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let project_id = project["project_id"].as_str().unwrap();

        let (status, branch) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/branches"),
            serde_json::json!({
                "name": "feature/cache",
                "baseBranch": "main",
                "baseCommit": "base123",
                "headCommit": "abc1234"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let branch_id = branch["branch_id"].as_str().unwrap();
        let (status, branches) = json_request_auth(
            router.clone(),
            "GET",
            &format!("/cairn-memory/projects/{project_id}/branches"),
            Value::Null,
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(branches.as_array().unwrap().len(), 1);
        let (status, worktree) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/worktrees"),
            serde_json::json!({
                "branchId": branch_id,
                "path": "/worktrees/cache",
                "commit": "abc1234"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(worktree["branch_id"], branch_id);
        let (status, worktrees) = json_request_auth(
            router.clone(),
            "GET",
            &format!("/cairn-memory/projects/{project_id}/worktrees"),
            Value::Null,
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(worktrees.as_array().unwrap().len(), 1);

        let (status, closable_branch) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/branches"),
            serde_json::json!({"name": "feature/obsolete", "baseBranch": "main"}),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let closable_branch_id = closable_branch["branch_id"].as_str().unwrap();
        let (status, closed_branch) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/branches/{closable_branch_id}/close"),
            serde_json::json!({"reason": "Superseded before merge"}),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(closed_branch["status"], "closed");

        let (_, source) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "The cache decision was tested on the feature branch.",
                "context": {
                    "repository": "cairn-demo",
                    "branch": "feature/cache",
                    "worktree": "wt-cache",
                    "commit": "abc1234",
                    "files": ["src/cache.rs"],
                    "symbols": ["Cache::get"],
                    "commands": ["cargo test cache"],
                    "tests": ["cache_reads_are_consistent"]
                }
            }),
        )
        .await;
        let source_id = source["memory_id"].as_str().unwrap();
        let (status, promoted) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/promote",
            serde_json::json!({
                "projectId": project_id,
                "memoryId": source_id,
                "reason": "Confirmed by the cache regression test"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(promoted["scope"], "project");
        assert_eq!(promoted["kind"], "decision");
        assert_eq!(promoted["project_id"], project_id);
        assert_eq!(promoted["promoted_from"], source_id);
        assert_eq!(promoted["verified_commit"], "abc1234");
        assert_eq!(promoted["context"]["branch"], Value::Null);
        assert_eq!(promoted["context"]["files"][0], "src/cache.rs");

        let (status, stale_results) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/search",
            serde_json::json!({
                "query": "cache decision",
                "limit": 10,
                "context": {"repository": "cairn-demo", "commit": "def5678"}
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let stale = stale_results
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["memory_id"] == promoted["memory_id"])
            .unwrap();
        assert_eq!(stale["stale"], true);
        assert_eq!(
            stale["stale_reason"],
            "project memory was verified at a different commit"
        );

        let (status, verified) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/projects/verify",
            serde_json::json!({
                "projectId": project_id,
                "memoryId": promoted["memory_id"],
                "commit": "def5678"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(verified["verified_commit"], "def5678");

        let (_, fresh_results) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/search",
            serde_json::json!({
                "query": "cache decision",
                "limit": 10,
                "context": {"repository": "cairn-demo", "commit": "def5678"}
            }),
        )
        .await;
        let fresh = fresh_results
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["memory_id"] == promoted["memory_id"])
            .unwrap();
        assert_eq!(fresh["stale"], false);

        let (status, project_memories) = json_request_auth(
            router.clone(),
            "GET",
            &format!("/cairn-memory/projects/{project_id}/memories"),
            Value::Null,
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(project_memories.as_array().unwrap().len(), 1);

        let (status, merged) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/branches/{branch_id}/merge"),
            serde_json::json!({
                "memoryId": source_id,
                "commit": "merge999",
                "reason": "Confirmed after merge into main"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(merged["branch"]["status"], "merged");
        assert_eq!(merged["branch"]["merge_commit"], "merge999");
        assert_eq!(merged["memory"]["memory_id"], promoted["memory_id"]);
        assert_eq!(merged["memory"]["verified_commit"], "merge999");

        let (status, resolved_branches) = json_request_auth(
            router.clone(),
            "GET",
            &format!("/cairn-memory/projects/{project_id}/branches"),
            Value::Null,
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(resolved_branches[0]["status"], "merged");
        let (status, resolved_worktrees) = json_request_auth(
            router.clone(),
            "GET",
            &format!("/cairn-memory/projects/{project_id}/worktrees"),
            Value::Null,
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(resolved_worktrees[0]["status"], "removed");

        let (status, repeated_merge) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/branches/{branch_id}/merge"),
            serde_json::json!({
                "memoryId": source_id,
                "commit": "merge999",
                "reason": "Confirmed after merge into main"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(repeated_merge["memory"]["memory_id"], promoted["memory_id"]);

        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/branches/{branch_id}/close"),
            serde_json::json!({"reason": "No longer needed"}),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, repeated) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/promote",
            serde_json::json!({
                "projectId": project_id,
                "memoryId": source_id,
                "reason": "Confirmed by the cache regression test"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(repeated["memory_id"], promoted["memory_id"]);

        let (status, secret) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "api_key=do-not-promote",
                "context": {"repository": "cairn-demo", "branch": "feature/cache"}
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/promote",
            serde_json::json!({
                "projectId": project_id,
                "memoryId": secret["memory_id"],
                "reason": "Should fail closed"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn project_registry_reports_entities_freshness_and_maintenance() {
        let path = temp_store_path("project-registry");
        let router = app(AppState::open(&path).unwrap());
        let (_, bootstrap) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({
                "email": "registry-owner@example.test",
                "password": "correct horse battery staple"
            }),
        )
        .await;
        let account_token = bootstrap["token"].as_str().unwrap();
        let (status, _) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/projects/missing-project/maintenance",
            serde_json::json!({ "currentCommit": "abc1234" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, project) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/projects",
            serde_json::json!({
                "repository": "cairn-demo",
                "name": "Cairn Demo",
                "defaultBranch": "main"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let project_id = project["project_id"].as_str().unwrap();

        for (method, path) in [
            ("GET", "/cairn-memory/projects/missing-project/entities"),
            ("GET", "/cairn-memory/projects/missing-project/freshness"),
            ("POST", "/cairn-memory/projects/missing-project/maintenance"),
            ("POST", "/cairn-memory/projects/missing-project/confirm"),
        ] {
            let body = if method == "GET" {
                Value::Null
            } else if path.ends_with("/confirm") {
                serde_json::json!({
                    "memoryId": "memory-missing",
                    "commit": "abc1234",
                    "reason": "missing project"
                })
            } else {
                serde_json::json!({ "currentCommit": "abc1234" })
            };
            let (status, _) =
                json_request_auth(router.clone(), method, path, body, account_token).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{method} {path}");
        }

        let (_, source) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "The cache decision was tested on the feature branch.",
                "context": {
                    "repository": "cairn-demo",
                    "branch": "feature/cache",
                    "commit": "abc1234",
                    "files": ["src/cache.rs"],
                    "symbols": ["Cache::get"],
                    "handoffs": ["cache-handoff"],
                    "runbooks": ["cache-runbook"]
                }
            }),
        )
        .await;
        let source_id = source["memory_id"].as_str().unwrap();
        let (status, confirmed) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/confirm"),
            serde_json::json!({
                "memoryId": source_id,
                "commit": "confirm123",
                "reason": "Confirmed for the registry"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(confirmed["scope"], "project");
        assert_eq!(confirmed["verified_commit"], "confirm123");
        assert_eq!(confirmed["lifecycle"]["freshness"], 100);
        let promoted_id = confirmed["memory_id"].as_str().unwrap();

        let (status, entities) = json_request_auth(
            router.clone(),
            "GET",
            &format!("/cairn-memory/projects/{project_id}/entities"),
            Value::Null,
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let entities = entities.as_array().unwrap();
        for (kind, key) in [
            ("decision", promoted_id),
            ("file", "src/cache.rs"),
            ("symbol", "Cache::get"),
            ("handoff", "cache-handoff"),
            ("runbook", "cache-runbook"),
        ] {
            assert!(
                entities
                    .iter()
                    .any(|entity| entity["kind"] == kind && entity["key"] == key),
                "missing {kind} entity {key}"
            );
        }

        let (status, registered) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/entities"),
            serde_json::json!({
                "kind": "runbook",
                "key": "deploy",
                "content": "api_key=do-not-store",
                "sourceMemoryId": source_id,
                "commit": "abc1234"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(registered["content"], "api_key=[REDACTED]");
        assert!(
            !registered["content"]
                .as_str()
                .unwrap()
                .contains("do-not-store")
        );
        let (status, repeated) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/entities"),
            serde_json::json!({
                "kind": "runbook",
                "key": "deploy",
                "content": "a later edit"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(repeated["entity_id"], registered["entity_id"]);
        assert_eq!(repeated["content"], "api_key=[REDACTED]");

        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/entities"),
            serde_json::json!({ "kind": "widget", "key": "nope" }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (_, other) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "This belongs to another repository.",
                "context": { "repository": "other-repo" }
            }),
        )
        .await;
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/entities"),
            serde_json::json!({
                "kind": "file",
                "key": "src/other.rs",
                "sourceMemoryId": other["memory_id"]
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, history) = json_request_auth(
            router.clone(),
            "GET",
            &format!("/cairn-memory/projects/{project_id}/freshness"),
            Value::Null,
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let history = history.as_array().unwrap();
        assert!(history.iter().any(|event| {
            event["action"] == "promotion"
                && event["commit"] == "abc1234"
                && event["memory_id"] == promoted_id
        }));
        assert!(history.iter().any(|event| {
            event["action"] == "verification"
                && event["commit"] == "confirm123"
                && event["previous_commit"] == "abc1234"
                && event["memory_id"] == promoted_id
        }));

        let (status, stale) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/maintenance"),
            serde_json::json!({ "currentCommit": "other999" }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(stale["current_commit"], "other999");
        assert!(
            stale["stale_memory_ids"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == promoted_id)
        );
        assert!(
            stale["active_memory_ids"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == promoted_id)
        );
        assert!(
            stale["superseded_memory_ids"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        let (status, current) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/maintenance"),
            serde_json::json!({ "currentCommit": "confirm123" }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            !current["stale_memory_ids"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == promoted_id)
        );

        let (status, _) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/forget",
            serde_json::json!({ "memoryId": promoted_id }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, after_forget) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/maintenance"),
            serde_json::json!({ "currentCommit": "other999" }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            !after_forget["active_memory_ids"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == promoted_id)
        );
        assert!(
            !after_forget["stale_memory_ids"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == promoted_id)
        );

        let (_, secret) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "api_key=do-not-promote",
                "context": { "repository": "cairn-demo", "commit": "abc1234" }
            }),
        )
        .await;
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/confirm"),
            serde_json::json!({
                "memoryId": secret["memory_id"],
                "commit": "confirm123",
                "reason": "Should fail closed"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn viewer_page_is_available_from_the_server() {
        let path = temp_store_path("viewer");
        let response = app(AppState::open(&path).unwrap())
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/viewer")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("Cairn / memory field"));
        assert!(body.contains("/cairn-memory/export"));
        assert!(body.contains("/cairn-memory/correct"));
        assert!(body.contains("Correct"));
        assert!(body.contains("Forget this"));
        assert!(body.contains("Current decisions"));
        assert!(body.contains("verified_commit"));
        assert!(body.contains("lifecycle.freshness"));
        assert!(body.contains("supersedes"));
        assert!(body.contains("Supersedes"));
        assert!(body.contains("Freshness history"));
        assert!(body.contains("\"action\""));
        assert!(body.contains("\"commit\""));
        assert!(body.contains("\"memory_id\""));
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn project_correction_is_visible_in_the_viewer_data_path() {
        let path = temp_store_path("project-correction-viewer");
        let router = app(AppState::open(&path).unwrap());
        let (_, bootstrap) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({
                "email": "correction-owner@example.test",
                "password": "correct horse battery staple"
            }),
        )
        .await;
        let account_token = bootstrap["token"].as_str().unwrap();
        let (_, project) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/projects",
            serde_json::json!({
                "repository": "cairn-demo",
                "name": "Cairn Demo",
                "defaultBranch": "main"
            }),
            account_token,
        )
        .await;
        let project_id = project["project_id"].as_str().unwrap();
        let (_, source) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "The cache decision was tested on the repository.",
                "context": {
                    "repository": "cairn-demo",
                    "commit": "abc1234"
                }
            }),
        )
        .await;
        let source_id = source["memory_id"].as_str().unwrap();
        let (status, promoted) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/promote",
            serde_json::json!({
                "projectId": project_id,
                "memoryId": source_id,
                "reason": "Confirmed for viewer inspection"
            }),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let promoted_id = promoted["memory_id"].as_str().unwrap();
        let (status, correction) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/correct",
            serde_json::json!({
                "targetId": promoted_id,
                "content": "The cache decision was confirmed after review."
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let correction_id = correction["memory_id"].as_str().unwrap();
        assert_eq!(correction["supersedes"], promoted_id);
        assert_eq!(correction["project_id"], project_id);

        let (status, memories) = json_request_auth(
            router.clone(),
            "GET",
            &format!("/cairn-memory/projects/{project_id}/memories"),
            Value::Null,
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let memories = memories.as_array().unwrap();
        assert!(
            memories
                .iter()
                .all(|memory| memory["memory_id"] != promoted_id)
        );
        assert!(
            memories
                .iter()
                .any(|memory| memory["memory_id"] == correction_id)
        );

        let (status, report) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/maintenance"),
            serde_json::json!({}),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            report["superseded_memory_ids"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == promoted_id)
        );
        assert!(
            report["active_memory_ids"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == correction_id)
        );

        let (status, history) = json_request_auth(
            router.clone(),
            "GET",
            &format!("/cairn-memory/projects/{project_id}/freshness"),
            Value::Null,
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(history.as_array().unwrap().iter().any(|event| {
            event["action"] == "promotion"
                && event["commit"] == "abc1234"
                && event["memory_id"] == promoted_id
        }));
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn capture_end_context_and_forget_work_over_http() {
        let path = temp_store_path("flow");
        let state = AppState::open(&path).unwrap();
        let router = app(state);

        let (status, _) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/session/start",
            serde_json::json!({
                "sessionId": "session-http",
                "context": {"repository": "cairn-demo", "branch": "main"}
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, capture) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/observe",
            serde_json::json!({
                "eventId": "event-http",
                "sessionId": "session-http",
                "content": "The authentication test passed.",
                "role": "agent",
                "source": "hook"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(capture["accepted"], true);

        let (status, memory) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/session/end",
            serde_json::json!({"sessionId": "session-http"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(memory["source_event_ids"][0], "event-http");

        let (status, context) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/context",
            serde_json::json!({"query": "authentication test", "limit": 5}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(context["results"][0]["source_event_ids"][0], "event-http");

        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/cairn-memory/forget")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"eventId":"event-http"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let (status, context) = json_request(
            app(AppState::open(&path).unwrap()),
            "POST",
            "/cairn-memory/context",
            serde_json::json!({"query": "authentication test", "limit": 5}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(context["results"].as_array().unwrap().len(), 0);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn malformed_requests_return_bad_request() {
        let path = temp_store_path("bad-request");
        let router = app(AppState::open(&path).unwrap());
        let (status, error) = json_request(
            router,
            "POST",
            "/cairn-memory/context",
            serde_json::json!({"query": "", "limit": 5}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(error["error"].as_str().unwrap().contains("query"));
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn explicit_remember_and_compatibility_aliases_work() {
        let path = temp_store_path("remember");
        let router = app(AppState::open(&path).unwrap());
        let (status, memory) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "The project uses signed cookies for browser sessions.",
                "concepts": ["authentication", "sessions"]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            memory["memory_id"]
                .as_str()
                .unwrap()
                .starts_with("memory-explicit-")
        );
        assert_eq!(memory["kind"], "explicit");

        let (status, results) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/smart-search",
            serde_json::json!({"query": "signed cookies", "limit": 5}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(results.as_array().unwrap().len(), 1);

        let (status, recall) = json_request(
            router,
            "POST",
            "/cairn-memory/recall",
            serde_json::json!({"query": "browser sessions", "limit": 5}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(recall["results"].as_array().unwrap().len(), 1);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn scoped_search_keeps_branch_memory_out_of_main_context() {
        let path = temp_store_path("scoped-search");
        let router = app(AppState::open(&path).unwrap());
        let (status, project) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "The project cache convention is documented.",
                "context": {
                    "repository": "cairn-demo",
                    "files": ["src/cache.rs"],
                    "symbols": ["Cache::get"],
                    "commands": ["cargo test cache"],
                    "tests": ["cache_reads_are_consistent"],
                    "services": ["cache"]
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, branch) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "The Redis cache experiment is branch-local.",
                "context": {
                    "repository": "cairn-demo",
                    "branch": "experiment/redis-cache",
                    "worktree": "wt-cache"
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, global) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "The user prefers evidence-backed cache decisions."
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, main_results) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/search",
            serde_json::json!({
                "query": "cache",
                "limit": 10,
                "context": {
                    "repository": "cairn-demo",
                    "branch": "main",
                    "worktree": "wt-main"
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let main_ids: Vec<_> = main_results
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|result| result["memory_id"].as_str())
            .collect();
        assert!(main_ids.contains(&project["memory_id"].as_str().unwrap()));
        assert!(main_ids.contains(&global["memory_id"].as_str().unwrap()));
        assert!(!main_ids.contains(&branch["memory_id"].as_str().unwrap()));
        let project_result = main_results
            .as_array()
            .unwrap()
            .iter()
            .find(|result| result["memory_id"] == project["memory_id"])
            .unwrap();
        assert_eq!(project_result["context"]["symbols"][0], "Cache::get");
        assert_eq!(project_result["context"]["commands"][0], "cargo test cache");
        assert_eq!(
            project_result["context"]["tests"][0],
            "cache_reads_are_consistent"
        );
        assert_eq!(project_result["context"]["services"][0], "cache");

        let (status, branch_results) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/context",
            serde_json::json!({
                "query": "cache",
                "limit": 2,
                "context": {
                    "repository": "cairn-demo",
                    "branch": "experiment/redis-cache",
                    "worktree": "wt-cache"
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(branch_results["results"].as_array().unwrap().len(), 2);
        assert_eq!(branch_results["omitted_count"], 1);
        assert!(branch_results["recovery_hint"].is_string());
        assert!(branch_results["omitted_memory_ids"].is_array());

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn resume_and_unresolved_routes_return_structured_handoff() {
        let path = temp_store_path("handoff");
        let router = app(AppState::open(&path).unwrap());
        let (status, _) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/session/start",
            serde_json::json!({
                "sessionId": "handoff-session",
                "context": {
                    "repository": "cairn-demo",
                    "branch": "feat/auth",
                    "worktree": "wt-auth"
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/observe",
            serde_json::json!({
                "eventId": "handoff-event",
                "sessionId": "handoff-session",
                "content": "The next step is to add the regression test; the issue is not yet verified.",
                "role": "agent",
                "source": "hook"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/session/end",
            serde_json::json!({"sessionId": "handoff-session"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let request = serde_json::json!({
            "limit": 5,
            "context": {
                "repository": "cairn-demo",
                "branch": "feat/auth",
                "worktree": "wt-auth"
            }
        });
        let (status, resume) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/resume",
            request.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(resume["results"].as_array().unwrap().len(), 1);
        assert_eq!(
            resume["results"][0]["next_steps"].as_array().unwrap().len(),
            1
        );

        let (status, unresolved) =
            json_request(router, "POST", "/cairn-memory/unresolved", request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            unresolved["results"][0]["unresolved_work"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn why_changed_route_returns_reason_relationships_and_conflicts() {
        let path = temp_store_path("why-changed");
        let router = app(AppState::open(&path).unwrap());
        let context = serde_json::json!({
            "repository": "cairn-demo",
            "files": ["src/cache.rs"],
            "symbols": ["Cache::get"],
            "commands": ["cargo test cache"],
            "tests": ["cache_reads_are_consistent"],
            "services": ["cache"]
        });
        let (status, first) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "The project uses Redis as the default cache because reads are fast.",
                "context": context
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, second) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "The project does not use Redis as the default cache because consistency is preferred.",
                "context": {"repository": "cairn-demo"}
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, pack) = json_request(
            router,
            "POST",
            "/cairn-memory/why-changed",
            serde_json::json!({"query":"Redis default cache","limit":10,"context":{"repository":"cairn-demo"}}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let first_result = pack["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|result| result["memory_id"] == first["memory_id"])
            .unwrap();
        let second_result = pack["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|result| result["memory_id"] == second["memory_id"])
            .unwrap();
        assert_eq!(first_result["decision_reason"], "reads are fast");
        assert!(
            first_result["relationships"]
                .as_array()
                .unwrap()
                .iter()
                .any(|relationship| {
                    relationship["relation"] == "file_contains_symbol"
                        && relationship["to"] == "Cache::get"
                })
        );
        assert_eq!(second_result["contradictory"], true);
        assert!(
            second_result["contradiction_ids"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == &first["memory_id"])
        );

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn correction_replaces_recall_without_destroying_provenance() {
        let path = temp_store_path("correction");
        let router = app(AppState::open(&path).unwrap());
        let (status, original) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({"content":"The default retention is seven days."}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let original_id = original["memory_id"].as_str().unwrap();

        let (status, corrected) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/correct",
            serde_json::json!({
                "targetId": original_id,
                "content":"The default retention is thirty days.",
                "lifecycle":{"confidence":95}
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(corrected["kind"], "correction");
        assert_eq!(corrected["supersedes"], original_id);
        assert_eq!(corrected["lifecycle"]["confidence"], 100);

        let (status, results) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/search",
            serde_json::json!({"query":"default retention", "limit":5}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(results.as_array().unwrap().len(), 1);
        assert_eq!(results[0]["memory_id"], corrected["memory_id"]);
        let (status, bundle) =
            json_request(router, "POST", "/cairn-memory/export", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            bundle["memories"]
                .as_array()
                .unwrap()
                .iter()
                .any(|memory| memory["memory_id"] == original_id)
        );
        assert!(
            bundle["forgottenIds"]
                .as_array()
                .unwrap()
                .iter()
                .all(|id| id != original_id)
        );
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn export_and_import_round_trip_over_http() {
        let source_path = temp_store_path("export-source");
        let target_path = temp_store_path("export-target");
        let source = app(AppState::open(&source_path).unwrap());
        let (status, _) = json_request(
            source.clone(),
            "POST",
            "/cairn-memory/session/start",
            serde_json::json!({"sessionId":"export-session"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = json_request(
            source.clone(),
            "POST",
            "/cairn-memory/observe",
            serde_json::json!({
                "eventId":"export-event",
                "sessionId":"export-session",
                "content":"Export and import preserves this evidence.",
                "role":"agent",
                "source":"test"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = json_request(
            source.clone(),
            "POST",
            "/cairn-memory/session/end",
            serde_json::json!({"sessionId":"export-session"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = json_request(
            source.clone(),
            "POST",
            "/cairn-memory/forget",
            serde_json::json!({"eventId":"export-event"}),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, bundle) =
            json_request(source, "POST", "/cairn-memory/export", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(bundle["format"], "cairn-memory");
        assert_eq!(bundle["version"], 1);

        let target = app(AppState::open(&target_path).unwrap());
        let (status, imported) = json_request(
            target.clone(),
            "POST",
            "/cairn-memory/import",
            serde_json::json!({"bundle": bundle}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(imported["imported_memories"], 1);
        assert_eq!(imported["imported_forgotten_ids"], 1);

        let (status, results) = json_request(
            target,
            "POST",
            "/cairn-memory/search",
            serde_json::json!({"query":"Export and import", "limit":5}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(results.as_array().unwrap().is_empty());
        let _ = std::fs::remove_file(source_path);
        let _ = std::fs::remove_file(target_path);
    }

    #[tokio::test]
    async fn lifecycle_fields_and_diagnostics_work_over_http() {
        let path = temp_store_path("lifecycle");
        let router = app(AppState::open(&path).unwrap());
        let (status, capture) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/observe",
            serde_json::json!({
                "eventId":"lifecycle-event",
                "sessionId":"lifecycle-session",
                "content":"Private password=secret-value decision",
                "role":"agent",
                "source":"test",
                "importance":"high",
                "lifecycle":{"confidence":90,"freshness":70}
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(capture["observation"]["lifecycle"]["importance"], 80);
        assert_eq!(capture["observation"]["lifecycle"]["sensitivity"], "secret");
        assert_eq!(capture["observation"]["lifecycle"]["confidence"], 90);

        let (status, memory) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content":"A temporary HTTP note",
                "lifecycle":{"retention":"temporary","expiresAt":"1"}
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(memory["lifecycle"]["retention"], "temporary");

        let (status, diagnose) =
            json_request(router, "GET", "/cairn-memory/diagnose", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(diagnose["temporary_observations"], 1);
        assert_eq!(diagnose["expired_items"], 1);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn explicit_project_share_is_readable_by_the_member_and_revocable() {
        let path = temp_store_path("project-share");
        let router = app(AppState::open(&path).unwrap());
        let password = "correct horse battery staple";
        let (_, owner) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({
                "email": "share-owner@example.test",
                "password": password
            }),
        )
        .await;
        let owner_token = owner["token"].as_str().unwrap();
        let (_, member) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/register",
            serde_json::json!({
                "email": "share-member@example.test",
                "password": password
            }),
        )
        .await;
        let member_token = member["token"].as_str().unwrap();
        let (_, project) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/projects",
            serde_json::json!({
                "repository": "cairn-demo",
                "name": "Cairn Demo",
                "defaultBranch": "main"
            }),
            owner_token,
        )
        .await;
        let project_id = project["project_id"].as_str().unwrap();
        let (_, private_memory) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "private observation stays on the owner account"
            }),
            owner_token,
        )
        .await;
        let private_id = private_memory["memory_id"].as_str().unwrap();
        let (_, repository_memory) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "repository cache decision is ready for the project",
                "context": {"repository": "cairn-demo", "commit": "abc1234"}
            }),
            owner_token,
        )
        .await;
        let repository_id = repository_memory["memory_id"].as_str().unwrap();
        let (_, promoted) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/promote",
            serde_json::json!({
                "projectId": project_id,
                "memoryId": repository_id,
                "reason": "Share the promoted repository decision"
            }),
            owner_token,
        )
        .await;
        let promoted_id = promoted["memory_id"].as_str().unwrap();
        let members = format!("/cairn-memory/projects/{project_id}/members");
        let shares = format!("/cairn-memory/projects/{project_id}/shares");
        let shared = format!("/cairn-memory/projects/{project_id}/shared");

        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &members,
            serde_json::json!({"email": "share-owner@example.test"}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &members,
            serde_json::json!({"email": "missing@example.test"}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, invited) = json_request_auth(
            router.clone(),
            "POST",
            &members,
            serde_json::json!({"email": "share-member@example.test"}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, repeated_invite) = json_request_auth(
            router.clone(),
            "POST",
            &members,
            serde_json::json!({"email": "share-member@example.test"}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(repeated_invite["person_id"], invited["person_id"]);

        let (status, denied) = json_request_auth(
            router.clone(),
            "POST",
            &shares,
            serde_json::json!({
                "memoryId": private_id,
                "email": "share-member@example.test"
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            denied["error"]
                .as_str()
                .unwrap()
                .contains("cannot be shared")
        );
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &shares,
            serde_json::json!({
                "memoryId": promoted_id,
                "email": "share-member@example.test"
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, shared_memories) =
            json_request_auth(router.clone(), "GET", &shared, Value::Null, member_token).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(shared_memories.as_array().unwrap().len(), 1);
        assert_eq!(shared_memories[0]["memory_id"], promoted_id);

        let (status, member_search) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/search",
            serde_json::json!({"query": "private observation", "limit": 5}),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(member_search.as_array().unwrap().is_empty());
        let (status, member_context) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/context",
            serde_json::json!({"query": "private observation", "limit": 5}),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(member_context["results"].as_array().unwrap().is_empty());

        let (status, owner_denied) = json_request_auth(
            router.clone(),
            "GET",
            &format!("/cairn-memory/projects/{project_id}/memories"),
            Value::Null,
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(
            owner_denied["error"]
                .as_str()
                .unwrap()
                .contains("does not own")
        );
        let (status, correct_denied) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/correct",
            serde_json::json!({
                "targetId": promoted_id,
                "content": "A member must not rewrite the shared decision."
            }),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(
            correct_denied["error"]
                .as_str()
                .unwrap()
                .contains("does not own")
        );
        let (status, forget_denied) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/forget",
            serde_json::json!({"id": promoted_id}),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(
            forget_denied["error"]
                .as_str()
                .unwrap()
                .contains("does not own")
        );

        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/shares/{promoted_id}/revoke"),
            serde_json::json!({"email": "share-member@example.test"}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, after_revoke) =
            json_request_auth(router.clone(), "GET", &shared, Value::Null, member_token).await;
        assert_eq!(status, StatusCode::OK);
        assert!(after_revoke.as_array().unwrap().is_empty());
        let (status, owner_memories) = json_request_auth(
            router.clone(),
            "GET",
            &format!("/cairn-memory/projects/{project_id}/memories"),
            Value::Null,
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            owner_memories
                .as_array()
                .unwrap()
                .iter()
                .any(|memory| memory["memory_id"] == promoted_id)
        );
        let (status, exported) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/export",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            exported["memories"]
                .as_array()
                .unwrap()
                .iter()
                .any(|memory| memory["memory_id"] == promoted_id)
        );
        let (status, audit) = json_request_auth(
            router.clone(),
            "GET",
            "/cairn-memory/auth/audit",
            Value::Null,
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let actions: Vec<_> = audit
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|event| event["action"].as_str())
            .collect();
        assert!(actions.contains(&"project_memory_shared"));
        assert!(actions.contains(&"project_memory_share_revoked"));
        assert!(audit.as_array().unwrap().iter().all(|event| {
            !event.to_string().contains("private observation")
                && !event.to_string().contains(password)
        }));

        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &shares,
            serde_json::json!({
                "memoryId": promoted_id,
                "email": "share-member@example.test"
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, successor) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/correct",
            serde_json::json!({
                "targetId": promoted_id,
                "content": "The successor repository decision replaces the shared one."
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let successor_id = successor["memory_id"].as_str().unwrap();
        let (status, omitted) =
            json_request_auth(router.clone(), "GET", &shared, Value::Null, member_token).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            omitted
                .as_array()
                .unwrap()
                .iter()
                .all(|memory| memory["memory_id"] != promoted_id)
        );
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &shares,
            serde_json::json!({
                "memoryId": successor_id,
                "email": "share-member@example.test"
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, successor_read) =
            json_request_auth(router.clone(), "GET", &shared, Value::Null, member_token).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            successor_read
                .as_array()
                .unwrap()
                .iter()
                .any(|memory| memory["memory_id"] == successor_id)
        );
        let _ = std::fs::remove_file(path);
    }
}
