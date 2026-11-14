//! REST compatibility transport for the Layer 1 Cairn core.

pub mod viewer;

use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use cairn_core::{
    Account, AuditEvent, AuthSessionResult, CaptureResult, ContextPack, DEVICE_TOKEN_PREFIX,
    Device, DeviceRegistration, ExportBundle, ImportResult, LifecycleInput, LifecycleSummary,
    Memory, MemoryKind, MemoryStore, ObservationInput, Project, ProjectBranch, ProjectConflict,
    ProjectEntity, ProjectEntityInput, ProjectFreshnessEvent, ProjectMaintenanceReport,
    ProjectMembership, ProjectMemoryShare, ProjectMergeResult, ProjectShareCandidate,
    ProjectSharePolicyRecord, ProjectTeam, ProjectTeamMember, ProjectWorktree, RecallFilter,
    RecallResult, Session, ShareCandidateDismissal, SharedProjectMemory, StoreError,
    SyncDiagnostics, SyncPullResult, SyncPushResult, WEB_ONLY_ERROR, WorkContext,
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
const AUTH_ATTEMPT_PRUNE_AT: usize = 1024;

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
        // Keys come from unauthenticated callers; drop expired windows so the map stays bounded.
        if attempts.len() >= AUTH_ATTEMPT_PRUNE_AT {
            attempts.retain(|_, (started, _)| now.duration_since(*started) < AUTH_ATTEMPT_WINDOW);
        }
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
    /// True while no account exists; the web console then offers first-account setup.
    pub bootstrap_required: bool,
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

#[derive(Serialize)]
struct ListedProject {
    #[serde(flatten)]
    project: Project,
    role: String,
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
pub struct ProjectTeamRequest {
    pub name: String,
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
pub struct ProjectPolicyRequest {
    pub policy: String,
}

#[derive(Clone, Debug, Deserialize)]
struct MemberEmailQuery {
    email: String,
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
    /// Defaults to the authenticated account; any other value is rejected.
    #[serde(rename = "personId", alias = "person_id", default)]
    pub person_id: Option<String>,
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
        let status = if error.to_string().contains(WEB_ONLY_ERROR) {
            StatusCode::FORBIDDEN
        } else if error.is_unauthorized() {
            StatusCode::UNAUTHORIZED
        } else {
            match &error {
                PostgresError::Validation(_) | PostgresError::Core(StoreError::Validation(_)) => {
                    StatusCode::BAD_REQUEST
                }
                PostgresError::Conflict(_) | PostgresError::Core(StoreError::Conflict(_)) => {
                    StatusCode::CONFLICT
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
                    StoreError::Validation(message) if message.contains(WEB_ONLY_ERROR) => {
                        StatusCode::FORBIDDEN
                    }
                    StoreError::Validation(_) => StatusCode::BAD_REQUEST,
                    StoreError::Conflict(_) => StatusCode::CONFLICT,
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
            "/cairn-memory/projects/{project_id}/team",
            get(list_project_team).post(create_project_team),
        )
        .route(
            "/cairn-memory/projects/{project_id}/team/members",
            post(add_project_team_member),
        )
        .route(
            "/cairn-memory/projects/{project_id}/team/members/remove",
            post(remove_project_team_member),
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
            "/cairn-memory/projects/{project_id}/policy",
            post(set_project_share_policy),
        )
        .route(
            "/cairn-memory/projects/{project_id}/share-candidates",
            get(list_share_candidates),
        )
        .route(
            "/cairn-memory/projects/{project_id}/share-candidates/{memory_id}/dismiss",
            post(dismiss_share_candidate),
        )
        .route(
            "/cairn-memory/projects/{project_id}/conflicts",
            get(list_project_conflicts),
        )
        .route(
            "/cairn-memory/projects/{project_id}/revocations",
            get(list_project_revocations),
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
        .route("/cairn-memory/delete", post(delete_memory))
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
        .layer(middleware::from_fn_with_state(state.clone(), touch_device))
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
            "/cairn-memory/projects/{project_id}/team",
            get(postgres_list_project_team).post(postgres_create_project_team),
        )
        .route(
            "/cairn-memory/projects/{project_id}/team/members",
            post(postgres_add_project_team_member),
        )
        .route(
            "/cairn-memory/projects/{project_id}/team/members/remove",
            post(postgres_remove_project_team_member),
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
            "/cairn-memory/projects/{project_id}/policy",
            post(postgres_set_project_share_policy),
        )
        .route(
            "/cairn-memory/projects/{project_id}/share-candidates",
            get(postgres_list_share_candidates),
        )
        .route(
            "/cairn-memory/projects/{project_id}/share-candidates/{memory_id}/dismiss",
            post(postgres_dismiss_share_candidate),
        )
        .route(
            "/cairn-memory/projects/{project_id}/conflicts",
            get(postgres_list_project_conflicts),
        )
        .route(
            "/cairn-memory/projects/{project_id}/revocations",
            get(postgres_list_project_revocations),
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
        .route("/cairn-memory/observe", post(postgres_capture_observation))
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
        .route("/cairn-memory/delete", post(postgres_delete_memory))
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

async fn postgres_health(
    State(state): State<PostgresAppState>,
) -> Result<Json<HealthResponse>, PostgresApiError> {
    Ok(Json(HealthResponse {
        ok: true,
        service: "cairn-server",
        storage: "postgresql-layer-3",
        semantic_search: "disabled",
        bootstrap_required: !state.store.has_accounts().await?,
    }))
}

async fn postgres_auth_bootstrap(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<AccountCredentialsRequest>,
) -> Result<Json<AuthSessionResult>, PostgresApiError> {
    postgres_reject_device_bearer(&state, &headers).await?;
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
    headers: HeaderMap,
    Json(request): Json<AccountCredentialsRequest>,
) -> Result<Json<AuthSessionResult>, PostgresApiError> {
    postgres_reject_device_bearer(&state, &headers).await?;
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
    headers: HeaderMap,
    Json(request): Json<AccountCredentialsRequest>,
) -> Result<Json<AuthSessionResult>, PostgresApiError> {
    postgres_reject_device_bearer(&state, &headers).await?;
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
) -> Result<Json<Vec<ListedProject>>, PostgresApiError> {
    let token = postgres_token(&headers)?;
    let projects = state.store.projects(token).await?;
    let mut listed = Vec::with_capacity(projects.len());
    for project in projects {
        let role = match state
            .store
            .project_memories(token, &project.project_id)
            .await
        {
            Ok(_) => "owner".to_owned(),
            Err(error) if error.to_string().contains("does not own") => "member".to_owned(),
            Err(error) => return Err(error.into()),
        };
        listed.push(ListedProject { project, role });
    }
    Ok(Json(listed))
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

async fn postgres_create_project_team(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<ProjectTeamRequest>,
) -> Result<Json<ProjectTeam>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .create_project_team(postgres_token(&headers)?, &project_id, &request.name)
            .await?,
    ))
}

async fn postgres_add_project_team_member(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<ProjectMemberRequest>,
) -> Result<Json<ProjectTeamMember>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .add_project_team_member(postgres_token(&headers)?, &project_id, &request.email)
            .await?,
    ))
}

async fn postgres_remove_project_team_member(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<ProjectMemberRequest>,
) -> Result<Json<ProjectTeamMember>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .remove_project_team_member(postgres_token(&headers)?, &project_id, &request.email)
            .await?,
    ))
}

async fn postgres_list_project_team(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<ProjectTeam>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .project_team(postgres_token(&headers)?, &project_id)
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
) -> Result<Json<Vec<SharedProjectMemory>>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .shared_project_memories(postgres_token(&headers)?, &project_id)
            .await?,
    ))
}

async fn postgres_set_project_share_policy(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<ProjectPolicyRequest>,
) -> Result<Json<ProjectSharePolicyRecord>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .set_project_share_policy(postgres_token(&headers)?, &project_id, &request.policy)
            .await?,
    ))
}

async fn postgres_list_share_candidates(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Query(query): Query<MemberEmailQuery>,
) -> Result<Json<Vec<ProjectShareCandidate>>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .share_candidates(postgres_token(&headers)?, &project_id, &query.email)
            .await?,
    ))
}

async fn postgres_dismiss_share_candidate(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path((project_id, memory_id)): Path<(String, String)>,
    Json(request): Json<ProjectShareRevokeRequest>,
) -> Result<Json<ShareCandidateDismissal>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .dismiss_share_candidate(
                postgres_token(&headers)?,
                &project_id,
                &memory_id,
                &request.email,
            )
            .await?,
    ))
}

async fn postgres_list_project_conflicts(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<ProjectConflict>>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .project_conflicts(postgres_token(&headers)?, &project_id)
            .await?,
    ))
}

async fn postgres_list_project_revocations(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<ProjectMemoryShare>>, PostgresApiError> {
    Ok(Json(
        state
            .store
            .project_revocations(postgres_token(&headers)?, &project_id)
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

/// A device capture is a sync push: it is the account's observation and a sync event.
async fn postgres_capture_observation(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<ObservationRequest>,
) -> Result<Json<CaptureResult>, PostgresApiError> {
    let pushed = state
        .store
        .sync_push(postgres_token(&headers)?, observation_input(request))
        .await?;
    Ok(Json(CaptureResult {
        accepted: pushed.accepted,
        duplicate: pushed.duplicate,
        observation: pushed.observation,
    }))
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

async fn postgres_delete_memory(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<ForgetRequest>,
) -> Result<StatusCode, PostgresApiError> {
    let id = request
        .id
        .or(request.target_id)
        .ok_or_else(|| PostgresApiError(PostgresError::Validation("id is required".to_owned())))?;
    state
        .store
        .delete_memory(postgres_token(&headers)?, &id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn postgres_register_device(
    State(state): State<PostgresAppState>,
    headers: HeaderMap,
    Json(request): Json<DeviceRegisterRequest>,
) -> Result<Json<DeviceRegistration>, PostgresApiError> {
    let device_id = request.device_id.unwrap_or_else(generated_device_id);
    let token = postgres_token(&headers)?;
    let person_id = match request.person_id {
        Some(person_id) => person_id,
        None => state.store.authenticate_session(token).await?.person_id,
    };
    Ok(Json(
        state
            .store
            .register_device(
                token,
                &device_id,
                &person_id,
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

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        service: "cairn-server",
        storage: "file-backed-layer-3-prototype",
        semantic_search: "disabled",
        bootstrap_required: !state.store.lock().await.has_accounts(),
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
    headers: HeaderMap,
    Json(request): Json<AccountCredentialsRequest>,
) -> Result<Json<AuthSessionResult>, ApiError> {
    reject_device_bearer(&*state.store.lock().await, &headers)?;
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
    headers: HeaderMap,
    Json(request): Json<AccountCredentialsRequest>,
) -> Result<Json<AuthSessionResult>, ApiError> {
    reject_device_bearer(&*state.store.lock().await, &headers)?;
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
    headers: HeaderMap,
    Json(request): Json<AccountCredentialsRequest>,
) -> Result<Json<AuthSessionResult>, ApiError> {
    reject_device_bearer(&*state.store.lock().await, &headers)?;
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
) -> Result<Json<Vec<ListedProject>>, ApiError> {
    let token = auth_token(&headers)?;
    let store = state.store.lock().await;
    let projects = store.projects(token).map_err(account_api_error)?;
    let mut listed = Vec::with_capacity(projects.len());
    for project in projects {
        let role = match store.project_memories(token, &project.project_id) {
            Ok(_) => "owner".to_owned(),
            Err(StoreError::Validation(message)) if message.contains("does not own") => {
                "member".to_owned()
            }
            Err(error) => return Err(account_api_error(error)),
        };
        listed.push(ListedProject { project, role });
    }
    Ok(Json(listed))
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

async fn create_project_team(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<ProjectTeamRequest>,
) -> Result<Json<ProjectTeam>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .create_project_team(auth_token(&headers)?, &project_id, &request.name)
            .map_err(account_api_error)?,
    ))
}

async fn add_project_team_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<ProjectMemberRequest>,
) -> Result<Json<ProjectTeamMember>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .add_project_team_member(auth_token(&headers)?, &project_id, &request.email)
            .map_err(account_api_error)?,
    ))
}

async fn remove_project_team_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<ProjectMemberRequest>,
) -> Result<Json<ProjectTeamMember>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .remove_project_team_member(auth_token(&headers)?, &project_id, &request.email)
            .map_err(account_api_error)?,
    ))
}

async fn list_project_team(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<ProjectTeam>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(
        store
            .project_team(auth_token(&headers)?, &project_id)
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
) -> Result<Json<Vec<SharedProjectMemory>>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(
        store
            .shared_project_memories(auth_token(&headers)?, &project_id)
            .map_err(account_api_error)?,
    ))
}

async fn set_project_share_policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Json(request): Json<ProjectPolicyRequest>,
) -> Result<Json<ProjectSharePolicyRecord>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .set_project_share_policy(auth_token(&headers)?, &project_id, &request.policy)
            .map_err(account_api_error)?,
    ))
}

async fn list_share_candidates(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
    Query(query): Query<MemberEmailQuery>,
) -> Result<Json<Vec<ProjectShareCandidate>>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(
        store
            .share_candidates(auth_token(&headers)?, &project_id, &query.email)
            .map_err(account_api_error)?,
    ))
}

async fn dismiss_share_candidate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((project_id, memory_id)): Path<(String, String)>,
    Json(request): Json<ProjectShareRevokeRequest>,
) -> Result<Json<ShareCandidateDismissal>, ApiError> {
    let mut store = state.store.lock().await;
    Ok(Json(
        store
            .dismiss_share_candidate(
                auth_token(&headers)?,
                &project_id,
                &memory_id,
                &request.email,
            )
            .map_err(account_api_error)?,
    ))
}

async fn list_project_conflicts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<ProjectConflict>>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(
        store
            .project_conflicts(auth_token(&headers)?, &project_id)
            .map_err(account_api_error)?,
    ))
}

async fn list_project_revocations(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(project_id): Path<String>,
) -> Result<Json<Vec<ProjectMemoryShare>>, ApiError> {
    let store = state.store.lock().await;
    Ok(Json(
        store
            .project_revocations(auth_token(&headers)?, &project_id)
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
    headers: HeaderMap,
    Json(request): Json<SessionStartRequest>,
) -> Result<Json<Session>, ApiError> {
    let mut store = state.store.lock().await;
    if let Some(token) = file_bearer(&store, &headers)? {
        Ok(Json(
            store
                .start_session_for_account(token, request.session_id, request.context)
                .map_err(account_api_error)?,
        ))
    } else {
        Ok(Json(
            store.start_session(request.session_id, request.context)?,
        ))
    }
}

async fn capture_observation(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ObservationRequest>,
) -> Result<Json<CaptureResult>, ApiError> {
    let mut store = state.store.lock().await;
    match file_bearer(&store, &headers)? {
        Some(token) => Ok(Json(
            store
                .capture_for_account(token, observation_input(request))
                .map_err(account_api_error)?,
        )),
        None => Ok(Json(store.capture_observation(observation_input(request))?)),
    }
}

async fn end_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SessionEndRequest>,
) -> Result<Json<Memory>, ApiError> {
    let mut store = state.store.lock().await;
    match file_bearer(&store, &headers)? {
        Some(token) => Ok(Json(
            store
                .end_session_for_account(token, &request.session_id)
                .map_err(account_api_error)?,
        )),
        None => Ok(Json(store.end_session(&request.session_id)?)),
    }
}

async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SearchRequest>,
) -> Result<Json<Vec<RecallResult>>, ApiError> {
    let store = state.store.lock().await;
    match file_bearer(&store, &headers)? {
        Some(token) => Ok(Json(
            store
                .search_for_account(
                    token,
                    &request.query,
                    request.limit,
                    &request.recall_filter(),
                )
                .map_err(account_api_error)?,
        )),
        None => Ok(Json(store.search_with_filter(
            &request.query,
            request.limit,
            &request.recall_filter(),
        )?)),
    }
}

async fn why_changed(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SearchRequest>,
) -> Result<Json<ContextPack>, ApiError> {
    let store = state.store.lock().await;
    match file_bearer(&store, &headers)? {
        Some(token) => Ok(Json(
            store
                .why_changed_for_account(
                    token,
                    &request.query,
                    request.limit,
                    &request.recall_filter(),
                )
                .map_err(account_api_error)?,
        )),
        None => Ok(Json(store.why_changed_context(
            &request.query,
            request.limit,
            &request.recall_filter(),
        )?)),
    }
}

async fn context(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<SearchRequest>,
) -> Result<Json<ContextPack>, ApiError> {
    let store = state.store.lock().await;
    match file_bearer(&store, &headers)? {
        Some(token) => Ok(Json(
            store
                .context_for_account(
                    token,
                    &request.query,
                    request.limit,
                    &request.recall_filter(),
                )
                .map_err(account_api_error)?,
        )),
        None => Ok(Json(store.context_with_filter(
            &request.query,
            request.limit,
            &request.recall_filter(),
        )?)),
    }
}

async fn resume(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ContinuityRequest>,
) -> Result<Json<ContextPack>, ApiError> {
    let store = state.store.lock().await;
    match file_bearer(&store, &headers)? {
        Some(token) => Ok(Json(
            store
                .resume_for_account(token, request.limit, &request.recall_filter())
                .map_err(account_api_error)?,
        )),
        None => Ok(Json(
            store.resume_context(request.limit, &request.recall_filter())?,
        )),
    }
}

async fn unresolved(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ContinuityRequest>,
) -> Result<Json<ContextPack>, ApiError> {
    let store = state.store.lock().await;
    match file_bearer(&store, &headers)? {
        Some(token) => Ok(Json(
            store
                .unresolved_for_account(token, request.limit, &request.recall_filter())
                .map_err(account_api_error)?,
        )),
        None => Ok(Json(
            store.unresolved_context(request.limit, &request.recall_filter())?,
        )),
    }
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
    match file_bearer(&store, &headers)? {
        Some(token) => {
            store
                .forget_for_account(token, &id)
                .map_err(account_api_error)?;
        }
        None => store.forget(&id)?,
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ForgetRequest>,
) -> Result<StatusCode, ApiError> {
    let id = request
        .id
        .or(request.target_id)
        .ok_or_else(|| ApiError::from(StoreError::Validation("id is required".to_owned())))?;
    let mut store = state.store.lock().await;
    match file_bearer(&store, &headers)? {
        Some(token) => {
            store
                .delete_for_account(token, &id)
                .map_err(account_api_error)?;
        }
        None => store.forget(&id)?,
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn remember(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RememberRequest>,
) -> Result<Json<Memory>, ApiError> {
    let mut store = state.store.lock().await;
    match file_bearer(&store, &headers)? {
        Some(token) => Ok(Json(
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
        )),
        None => Ok(Json(store.remember_with_kind(
            &request.content,
            request.concepts,
            request.context,
            request.lifecycle,
            request.kind,
        )?)),
    }
}

async fn correct(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CorrectRequest>,
) -> Result<Json<Memory>, ApiError> {
    let mut store = state.store.lock().await;
    if let Some(token) = file_bearer(&store, &headers)? {
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

async fn export(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ExportBundle>, ApiError> {
    let store = state.store.lock().await;
    match file_bearer(&store, &headers)? {
        Some(token) => Ok(Json(
            store.export_for_account(token).map_err(account_api_error)?,
        )),
        None => Ok(Json(store.export())),
    }
}

async fn import(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ImportRequest>,
) -> Result<Json<ImportResult>, ApiError> {
    let mut store = state.store.lock().await;
    match file_bearer(&store, &headers)? {
        Some(token) => Ok(Json(
            store
                .import_for_account(token, request.bundle)
                .map_err(account_api_error)?,
        )),
        None => Ok(Json(store.import_bundle(request.bundle)?)),
    }
}

async fn diagnose(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<LifecycleSummary>, ApiError> {
    let store = state.store.lock().await;
    match file_bearer(&store, &headers)? {
        Some(token) => Ok(Json(
            store
                .lifecycle_summary_for_account(token)
                .map_err(account_api_error)?,
        )),
        None => Ok(Json(store.lifecycle_summary())),
    }
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
        .authenticate_session(auth_token(&headers)?)
        .map_err(account_api_error)?;
    if request
        .person_id
        .is_some_and(|person_id| account.person_id != person_id)
    {
        return Err(ApiError::Unauthorized(
            "account does not own the requested person".to_owned(),
        ));
    }
    Ok(Json(
        store
            .register_device(device_id, account.person_id, request.agent_id, request.name)
            .map_err(device_api_error)?,
    ))
}

async fn list_devices(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Device>>, ApiError> {
    let token = auth_token(&headers)?;
    let store = state.store.lock().await;
    let caller = store
        .authenticate_session(token)
        .map_err(account_api_error)?;
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
            .map_err(account_api_error)?,
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
            .map_err(account_api_error)?,
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

fn reject_device_bearer(store: &MemoryStore, headers: &HeaderMap) -> Result<(), ApiError> {
    if optional_auth_token(headers).is_some_and(|token| store.is_device_token(token)) {
        return Err(StoreError::Validation(WEB_ONLY_ERROR.to_owned()).into());
    }
    Ok(())
}

async fn postgres_reject_device_bearer(
    state: &PostgresAppState,
    headers: &HeaderMap,
) -> Result<(), PostgresApiError> {
    if let Some(token) = optional_auth_token(headers)
        && state.store.is_device_token(token).await?
    {
        return Err(PostgresError::Validation(WEB_ONLY_ERROR.to_owned()).into());
    }
    Ok(())
}

/// Records device use before the handler runs. File-store only: PostgreSQL records it
/// while authenticating.
async fn touch_device(State(state): State<AppState>, request: Request, next: Next) -> Response {
    // Session and malformed bearers cannot be device tokens, so they skip the global lock.
    if let Some(token) = optional_auth_token(request.headers())
        && token.starts_with(DEVICE_TOKEN_PREFIX)
    {
        state.store.lock().await.touch_device(token);
    }
    next.run(request).await
}

fn file_bearer<'a>(
    store: &MemoryStore,
    headers: &'a HeaderMap,
) -> Result<Option<&'a str>, ApiError> {
    if store.has_accounts() {
        auth_token(headers).map(Some)
    } else {
        Ok(optional_auth_token(headers))
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
                || message.contains("device is revoked")
                || message.contains("expired or revoked")
                || message.contains("account is disabled")
                || message.contains("invalid account credentials")
                || message.contains("device is outside the caller scope")
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
                || message.contains("device is revoked")
                || message.contains("device is outside the caller scope") =>
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

    #[test]
    fn a_postgres_conflict_is_http_409() {
        let response =
            PostgresApiError(PostgresError::Conflict("taken".to_owned())).into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

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

    #[test]
    fn auth_rate_limiter_drops_expired_windows() {
        let limiter = AuthRateLimiter::default();
        let stale = Instant::now() - AUTH_ATTEMPT_WINDOW - Duration::from_secs(1);
        {
            let mut attempts = limiter.attempts.lock().unwrap();
            for index in 0..AUTH_ATTEMPT_PRUNE_AT * 2 {
                attempts.insert(format!("login:{index}@example.test"), (stale, 1));
            }
        }
        assert!(limiter.allow("login:fresh@example.test"));
        assert_eq!(limiter.attempts.lock().unwrap().len(), 1);
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

        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/devices/rotate",
            serde_json::json!({"deviceId": "device-a"}),
            &first_token,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, rotated) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/devices/rotate",
            serde_json::json!({"deviceId": "device-a"}),
            account_token,
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

        // Registering the same device id again issues a new token; the old one stops working.
        let (status, again) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/devices/register",
            serde_json::json!({"deviceId": "device-a", "personId": person_id, "name": "MacBook"}),
            account_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let again_token = again["token"].as_str().unwrap();
        assert_ne!(again_token, rotated_token);
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/sync/pull",
            serde_json::json!({"cursor": 0}),
            rotated_token,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/sync/pull",
            serde_json::json!({"cursor": 0}),
            again_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, revoked) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/devices/revoke",
            serde_json::json!({"deviceId": "device-b"}),
            account_token,
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
    async fn file_device_token_is_an_agent_credential_and_web_only_routes_refuse_it() {
        let path = temp_store_path("device-agent-http");
        let router = app(AppState::open(&path).unwrap());
        let password = "correct horse battery staple";
        let (_, health) =
            json_request(router.clone(), "GET", "/cairn-memory/health", Value::Null).await;
        assert_eq!(health["bootstrap_required"], true);
        let (_, owner) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({"email": "agent-owner@example.test", "password": password}),
        )
        .await;
        let session = owner["token"].as_str().unwrap();
        let (_, health) =
            json_request(router.clone(), "GET", "/cairn-memory/health", Value::Null).await;
        assert_eq!(health["bootstrap_required"], false);
        let (_, other) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/register",
            serde_json::json!({"email": "agent-other@example.test", "password": password}),
        )
        .await;
        let other_session = other["token"].as_str().unwrap();

        // The session creates a device without naming the person; the token is shown once.
        let (status, created) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/devices/register",
            serde_json::json!({"name": "Laptop"}),
            session,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let device = created["token"].as_str().unwrap();
        let device_id = created["device"]["device_id"].as_str().unwrap();
        let (_, other_created) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/devices/register",
            serde_json::json!({"name": "Other"}),
            other_session,
        )
        .await;
        let other_device = other_created["token"].as_str().unwrap();
        let (status, listed) = json_request_auth(
            router.clone(),
            "GET",
            "/cairn-memory/devices",
            Value::Null,
            session,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert!(!listed.to_string().contains(device));

        // Agent routes accept the device token and act as its account.
        let (status, project) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/projects",
            serde_json::json!({"repository": "cairn-demo", "name": "Demo"}),
            device,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let project_id = project["project_id"].as_str().unwrap();
        let (status, projects) = json_request_auth(
            router.clone(),
            "GET",
            "/cairn-memory/projects",
            Value::Null,
            session,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(projects.as_array().unwrap().len(), 1);
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/search",
            serde_json::json!({"query": "anything"}),
            device,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = json_request_auth(
            router.clone(),
            "GET",
            &format!("/cairn-memory/projects/{project_id}/memories"),
            Value::Null,
            other_device,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Web-only routes refuse the device token with 403.
        for (method, route) in [
            ("GET", "/cairn-memory/devices"),
            ("POST", "/cairn-memory/devices/register"),
            ("POST", "/cairn-memory/devices/revoke"),
            ("POST", "/cairn-memory/devices/rotate"),
            ("GET", "/cairn-memory/auth/audit"),
            ("POST", "/cairn-memory/auth/logout"),
            ("POST", "/cairn-memory/auth/login"),
            ("POST", "/cairn-memory/auth/register"),
            ("POST", "/cairn-memory/auth/bootstrap"),
        ] {
            let (status, body) = json_request_auth(
                router.clone(),
                method,
                route,
                serde_json::json!({
                    "name": "x", "deviceId": device_id,
                    "email": "agent-third@example.test", "password": password
                }),
                device,
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{route}: {body}");
            assert!(body["error"].as_str().unwrap().contains("web console"));
        }

        // Merge: allowed on a solo project, 403 once the project has another member.
        let mut merge_ids = Vec::new();
        for name in ["feat/solo", "feat/shared"] {
            let (status, branch) = json_request_auth(
                router.clone(),
                "POST",
                &format!("/cairn-memory/projects/{project_id}/branches"),
                serde_json::json!({"name": name}),
                device,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let (status, memory) = json_request_auth(
                router.clone(),
                "POST",
                "/cairn-memory/remember",
                serde_json::json!({
                    "content": format!("decision on {name}"),
                    "context": {"repository": "cairn-demo", "branch": name}
                }),
                device,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            merge_ids.push((
                branch["branch_id"].as_str().unwrap().to_owned(),
                memory["memory_id"].as_str().unwrap().to_owned(),
            ));
        }
        let merge = |token: &str, ids: (String, String)| {
            let router = router.clone();
            let token = token.to_owned();
            let route = format!(
                "/cairn-memory/projects/{project_id}/branches/{}/merge",
                ids.0
            );
            let body = serde_json::json!({"memoryId": ids.1, "commit": "c1", "reason": "ok"});
            async move { json_request_auth(router, "POST", &route, body, &token).await }
        };
        let (status, body) = merge(device, merge_ids[0].clone()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/members"),
            serde_json::json!({"email": "agent-other@example.test"}),
            device,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) = merge(device, merge_ids[1].clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        let (status, body) = merge(session, merge_ids[1].clone()).await;
        assert_eq!(status, StatusCode::OK, "{body}");

        // Removing a member is web-only, so a device cannot reopen the solo-merge path.
        for (route, body) in [
            ("team", serde_json::json!({"name": "Core"})),
            (
                "team/members",
                serde_json::json!({"email": "agent-other@example.test"}),
            ),
        ] {
            let (status, body) = json_request_auth(
                router.clone(),
                "POST",
                &format!("/cairn-memory/projects/{project_id}/{route}"),
                body,
                device,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{route}: {body}");
        }
        let remove = |token: &str| {
            let router = router.clone();
            let token = token.to_owned();
            let route = format!("/cairn-memory/projects/{project_id}/team/members/remove");
            async move {
                json_request_auth(
                    router,
                    "POST",
                    &route,
                    serde_json::json!({"email": "agent-other@example.test"}),
                    &token,
                )
                .await
            }
        };
        let (status, body) = remove(device).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        let (status, body) = remove(session).await;
        assert_eq!(status, StatusCode::OK, "{body}");

        // A revoked device token is rejected on agent routes and on sync.
        let (status, revoked) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/devices/revoke",
            serde_json::json!({"deviceId": device_id}),
            session,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(revoked["status"], "revoked");
        for (route, body) in [
            ("/cairn-memory/search", serde_json::json!({"query": "x"})),
            (
                "/cairn-memory/remember",
                serde_json::json!({"content": "x"}),
            ),
            ("/cairn-memory/sync/pull", serde_json::json!({"cursor": 0})),
        ] {
            let (status, body) =
                json_request_auth(router.clone(), "POST", route, body, device).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{route}: {body}");
        }
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn touch_middleware_skips_the_store_lock_for_non_device_bearers() {
        let path = temp_store_path("touch-lock");
        let state = AppState::open(&path).unwrap();
        let router = app(state.clone());
        let _held = state.store.lock().await;
        let request = |token: &str| {
            Request::builder()
                .uri("/cairn-memory/no-such-route")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap()
        };
        let timeout = std::time::Duration::from_millis(300);
        let session = tokio::time::timeout(timeout, router.clone().oneshot(request("session-x")))
            .await
            .expect("a non-device bearer does not wait for the store lock");
        assert_eq!(session.unwrap().status(), StatusCode::NOT_FOUND);
        // A device-shaped bearer still takes the lock.
        let device = tokio::time::timeout(
            timeout,
            router.oneshot(request(&format!("{DEVICE_TOKEN_PREFIX}x"))),
        )
        .await;
        assert!(device.is_err());
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn file_sync_adopts_an_owned_observation_once() {
        let path = temp_store_path("sync-adopt");
        let router = app(AppState::open(&path).unwrap());
        let password = "correct horse battery staple";
        let (_, owner) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({
                "email": "sync-owner@example.test",
                "password": password
            }),
        )
        .await;
        let owner_token = owner["token"].as_str().unwrap();
        let owner_person = owner["account"]["person_id"].as_str().unwrap();
        let (_, member) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/register",
            serde_json::json!({
                "email": "sync-member@example.test",
                "password": password
            }),
        )
        .await;
        let member_token = member["token"].as_str().unwrap();
        let member_person = member["account"]["person_id"].as_str().unwrap();
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/session/start",
            serde_json::json!({
                "sessionId": "adopt-session",
                "context": {"repository": "owner-repo", "branch": "owner-branch"}
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, captured) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/observe",
            serde_json::json!({
                "eventId": "owned-event",
                "sessionId": "adopt-session",
                "content": "canonicalalphafact",
                "role": "agent",
                "source": "test"
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(captured["duplicate"], false);
        assert_eq!(captured["observation"]["context"]["branch"], "owner-branch");

        let (status, owner_device) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/devices/register",
            serde_json::json!({
                "deviceId": "device-owner",
                "personId": owner_person,
                "name": "Owner"
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let owner_device_token = owner_device["token"].as_str().unwrap();
        let (status, second_device) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/devices/register",
            serde_json::json!({
                "deviceId": "device-owner-2",
                "personId": owner_person,
                "name": "Owner two"
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let second_token = second_device["token"].as_str().unwrap();
        let (status, member_device) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/devices/register",
            serde_json::json!({
                "deviceId": "device-member",
                "personId": member_person,
                "name": "Member"
            }),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let member_device_token = member_device["token"].as_str().unwrap();

        let replacement = serde_json::json!({
            "eventId": "owned-event",
            "sessionId": "other-session",
            "content": "replacementbetatext",
            "role": "user",
            "source": "sync",
            "context": {"repository": "member-repo", "branch": "member-branch"}
        });
        let (status, adopted) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/sync/push",
            replacement.clone(),
            owner_device_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(adopted["accepted"], true);
        assert_eq!(adopted["duplicate"], false);
        assert_eq!(adopted["observation"]["content"], "canonicalalphafact");
        assert_eq!(adopted["observation"]["session_id"], "adopt-session");
        assert_eq!(adopted["observation"]["context"]["branch"], "owner-branch");
        let cursor = adopted["cursor"].clone();

        let (status, pulled) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/sync/pull",
            serde_json::json!({"cursor": 0}),
            second_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let events = pulled["events"].as_array().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event["event_id"] == "owned-event"
                    && event["observation"]["content"] == "canonicalalphafact")
                .count(),
            1
        );
        let (status, resumed) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/sync/pull",
            serde_json::json!({"cursor": pulled["cursor"]}),
            second_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            resumed["events"]
                .as_array()
                .unwrap()
                .iter()
                .all(|event| event["event_id"] != "owned-event")
        );

        let (status, retry) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/sync/push",
            replacement,
            owner_device_token,
        )
        .await;
        // The pushed text differs from the canonical row, so replaying it conflicts.
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(retry.get("observation").is_none(), "{retry}");
        let (status, retry) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/sync/push",
            serde_json::json!({
                "eventId": "owned-event",
                "sessionId": "adopt-session",
                "content": "canonicalalphafact",
                "role": "agent",
                "source": "test"
            }),
            owner_device_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(retry["duplicate"], true);
        assert_eq!(retry["cursor"], cursor);
        assert_eq!(retry["observation"]["content"], "canonicalalphafact");

        let (status, collision) = json_request_auth(
            router,
            "POST",
            "/cairn-memory/sync/push",
            serde_json::json!({
                "eventId": "owned-event",
                "sessionId": "adopt-session",
                "content": "memberstealtext",
                "role": "agent",
                "source": "sync"
            }),
            member_device_token,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let error = collision.to_string();
        assert!(
            error.contains("event belongs to another person: owned-event"),
            "{error}"
        );
        assert!(!error.contains("canonicalalphafact"), "{error}");
        assert!(!error.contains("memberstealtext"), "{error}");
        assert!(collision.get("observation").is_none(), "{collision}");
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

        let (_, source) = json_request_auth(
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
            account_token,
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

        let (status, stale_results) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/search",
            serde_json::json!({
                "query": "cache decision",
                "limit": 10,
                "context": {"repository": "cairn-demo", "commit": "def5678"}
            }),
            account_token,
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

        let (_, fresh_results) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/search",
            serde_json::json!({
                "query": "cache decision",
                "limit": 10,
                "context": {"repository": "cairn-demo", "commit": "def5678"}
            }),
            account_token,
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
        assert_eq!(merged["memory"]["promoted_from"], source_id);
        assert!(
            merged["memory"]["source_event_ids"]
                .as_array()
                .unwrap()
                .is_empty(),
            "a promoted copy keeps no personal source events"
        );

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

        let (status, secret) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "api_key=do-not-promote",
                "context": {"repository": "cairn-demo", "branch": "feature/cache"}
            }),
            account_token,
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

        let (_, source) = json_request_auth(
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
            account_token,
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

        let (_, other) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "This belongs to another repository.",
                "context": { "repository": "other-repo" }
            }),
            account_token,
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

        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/forget",
            serde_json::json!({ "memoryId": promoted_id }),
            account_token,
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

        let (_, secret) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "api_key=do-not-promote",
                "context": { "repository": "cairn-demo", "commit": "abc1234" }
            }),
            account_token,
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
        assert!(body.contains("Invite member"));
        assert!(body.contains("Share decision"));
        assert!(body.contains("Revoke share"));
        assert!(body.contains("Create team"));
        assert!(body.contains("Add member"));
        assert!(body.contains("Remove member"));
        assert!(body.contains("Delete memory"));
        assert!(body.contains("/shared"));
        let memories_fn = body
            .split("async function loadProjectMemories")
            .nth(1)
            .expect("member memory loader");
        let member_at = memories_fn.find("if (member)").expect("member branch");
        let shared_at = memories_fn.find("/shared").expect("shared read");
        let owner_at = memories_fn.find("/memories").expect("owner read");
        assert!(member_at < shared_at && shared_at < owner_at);
        assert!(!memories_fn[member_at..shared_at].contains("/memories"));
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
        let (_, source) = json_request_auth(
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
            account_token,
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
        let (status, correction) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/correct",
            serde_json::json!({
                "targetId": promoted_id,
                "content": "The cache decision was confirmed after review."
            }),
            account_token,
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

    async fn post_as(
        router: &Router,
        token: &str,
        route: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        json_request_auth(router.clone(), "POST", route, body, token).await
    }

    #[tokio::test]
    async fn foreign_supersedes_import_does_not_hide_another_accounts_memory_over_http() {
        let path = temp_store_path("foreign-supersedes-http");
        let router = app(AppState::open(&path).unwrap());
        let (_, owner) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({"email":"owner@example.test","password":"correct horse battery staple"}),
        )
        .await;
        let (_, other) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/register",
            serde_json::json!({"email":"other@example.test","password":"correct horse battery staple"}),
        )
        .await;
        let owner_token = owner["token"].as_str().unwrap().to_owned();
        let other_token = other["token"].as_str().unwrap().to_owned();

        let (_, kept) = post_as(
            &router,
            &owner_token,
            "/cairn-memory/remember",
            serde_json::json!({"content":"alphalink checkout cache redis fits"}),
        )
        .await;
        let (_, target) = post_as(
            &router,
            &owner_token,
            "/cairn-memory/remember",
            serde_json::json!({"content":"alphalink checkout cache avoid redis"}),
        )
        .await;
        let target_id = target["memory_id"].as_str().unwrap().to_owned();
        post_as(
            &router,
            &owner_token,
            "/cairn-memory/session/start",
            serde_json::json!({"sessionId":"link-session"}),
        )
        .await;
        post_as(&router, &owner_token, "/cairn-memory/observe", serde_json::json!({"eventId":"link-evt","sessionId":"link-session","content":"resumelink resumefact","role":"agent","source":"test"})).await;
        let (_, summary) = post_as(
            &router,
            &owner_token,
            "/cairn-memory/session/end",
            serde_json::json!({"sessionId":"link-session"}),
        )
        .await;
        let summary_id = summary["memory_id"].as_str().unwrap().to_owned();

        // The other account imports its own rows naming the owner's rows as predecessors.
        let (status, mut bundle) =
            post_as(&router, &other_token, "/cairn-memory/export", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        let link = |label: &str, supersedes: &str| {
            let mut row = target.clone();
            row["memory_id"] = format!("memory-link-{label}").into();
            row["source_event_ids"] = serde_json::json!([format!("event-link-{label}")]);
            row["content"] = format!("betalink {label} unrelated").into();
            row["supersedes"] = supersedes.into();
            row
        };
        bundle["memories"] =
            serde_json::json!([link("target", &target_id), link("summary", &summary_id)]);
        let (status, imported) = post_as(
            &router,
            &other_token,
            "/cairn-memory/import",
            serde_json::json!({"bundle": bundle}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(imported["imported_memories"], 2);

        let ids = |value: &Value| {
            let mut ids = value
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["memory_id"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>();
            ids.sort();
            ids
        };
        let mut expected = vec![
            kept["memory_id"].as_str().unwrap().to_owned(),
            target_id.clone(),
        ];
        expected.sort();
        let (_, hits) = post_as(
            &router,
            &owner_token,
            "/cairn-memory/search",
            serde_json::json!({"query":"alphalink","limit":10}),
        )
        .await;
        assert_eq!(ids(&hits), expected);
        assert!(
            hits.as_array()
                .unwrap()
                .iter()
                .all(|hit| hit["contradictory"] == true)
        );
        let (_, pack) = post_as(
            &router,
            &owner_token,
            "/cairn-memory/context",
            serde_json::json!({"query":"alphalink","limit":10}),
        )
        .await;
        assert_eq!(ids(&pack["results"]), expected);
        let (_, resume) = post_as(
            &router,
            &owner_token,
            "/cairn-memory/resume",
            serde_json::json!({"limit":10}),
        )
        .await;
        assert_eq!(ids(&resume["results"]), vec![summary_id.clone()]);

        let (_, theirs) = post_as(
            &router,
            &other_token,
            "/cairn-memory/search",
            serde_json::json!({"query":"alphalink","limit":10}),
        )
        .await;
        assert!(theirs.as_array().unwrap().is_empty());
        let (_, own) = post_as(
            &router,
            &other_token,
            "/cairn-memory/context",
            serde_json::json!({"query":"betalink","limit":10}),
        )
        .await;
        assert_eq!(own["results"].as_array().unwrap().len(), 2);
        assert!(!own.to_string().contains("alphalink"));

        let (_, owner_export) =
            post_as(&router, &owner_token, "/cairn-memory/export", Value::Null).await;
        assert!(
            owner_export["memories"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["memory_id"] == target_id.as_str())
        );
        let (_, other_export) =
            post_as(&router, &other_token, "/cairn-memory/export", Value::Null).await;
        assert!(
            other_export["memories"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| {
                    row["memory_id"] == "memory-link-target"
                        && row["supersedes"] == target_id.as_str()
                })
        );
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn foreign_tombstone_import_is_rejected_and_unknown_ones_are_ignored_over_http() {
        let path = temp_store_path("foreign-tombstone-http");
        let router = app(AppState::open(&path).unwrap());
        let (_, owner) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({"email":"owner@example.test","password":"correct horse battery staple"}),
        )
        .await;
        let (_, other) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/register",
            serde_json::json!({"email":"other@example.test","password":"correct horse battery staple"}),
        )
        .await;
        let owner_token = owner["token"].as_str().unwrap().to_owned();
        let other_token = other["token"].as_str().unwrap().to_owned();
        let (_, kept) = post_as(
            &router,
            &owner_token,
            "/cairn-memory/remember",
            serde_json::json!({"content":"tombhttp private owner note"}),
        )
        .await;
        let kept_id = kept["memory_id"].as_str().unwrap().to_owned();
        let before = std::fs::read(&path).unwrap();

        let (_, mut bundle) =
            post_as(&router, &other_token, "/cairn-memory/export", Value::Null).await;
        bundle["memories"] = serde_json::json!([]);
        bundle["forgottenIds"] = serde_json::json!([kept_id]);
        let (status, error) = post_as(
            &router,
            &other_token,
            "/cairn-memory/import",
            serde_json::json!({"bundle": bundle}),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(!error.to_string().contains("tombhttp"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let (_, hits) = post_as(
            &router,
            &owner_token,
            "/cairn-memory/search",
            serde_json::json!({"query":"tombhttp","limit":5}),
        )
        .await;
        assert_eq!(hits.as_array().unwrap().len(), 1);

        bundle["forgottenIds"] = serde_json::json!(["tombhttp-unknown-id"]);
        let (status, imported) = post_as(
            &router,
            &other_token,
            "/cairn-memory/import",
            serde_json::json!({"bundle": bundle}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(imported["imported_forgotten_ids"], 0);

        // A memory row named like the owner's future event id does not poison its capture.
        let mut squat = kept.clone();
        squat["memory_id"] = "tombhttp-future-evt".into();
        squat["source_event_ids"] = serde_json::json!(["event-squat"]);
        squat["content"] = "tombhttp squat row".into();
        bundle["memories"] = serde_json::json!([squat]);
        bundle["forgottenIds"] = serde_json::json!(["tombhttp-future-evt"]);
        let (status, _) = post_as(
            &router,
            &other_token,
            "/cairn-memory/import",
            serde_json::json!({"bundle": bundle}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, error) = post_as(
            &router,
            &owner_token,
            "/cairn-memory/observe",
            serde_json::json!({"eventId":"tombhttp-future-evt","sessionId":"s","content":"tombhttp private capture","role":"agent","source":"test"}),
        )
        .await;
        assert_ne!(status, StatusCode::OK);
        assert!(
            error
                .to_string()
                .contains("event belongs to another person")
        );
        assert!(!error.to_string().contains("private capture"));
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn project_linked_import_is_owner_only_over_http() {
        let path = temp_store_path("project-import-http");
        let router = app(AppState::open(&path).unwrap());
        let password = "correct horse battery staple";
        let (_, owner) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({"email":"owner@example.test","password":password}),
        )
        .await;
        let (_, other) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/register",
            serde_json::json!({"email":"other@example.test","password":password}),
        )
        .await;
        let owner_token = owner["token"].as_str().unwrap().to_owned();
        let other_token = other["token"].as_str().unwrap().to_owned();
        let (_, project) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/projects",
            serde_json::json!({"repository":"cairn-demo","name":"Demo"}),
            &owner_token,
        )
        .await;
        let project_id = project["project_id"].as_str().unwrap().to_owned();
        let (_, template) = post_as(
            &router,
            &other_token,
            "/cairn-memory/remember",
            serde_json::json!({"content":"projhttp private claim"}),
        )
        .await;
        let before = std::fs::read(&path).unwrap();
        let mut row = template.clone();
        row["memory_id"] = "projhttp-row".into();
        row["source_event_ids"] = serde_json::json!([]);
        row["scope"] = "project".into();
        row["project_id"] = project_id.clone().into();
        let (_, mut bundle) =
            post_as(&router, &other_token, "/cairn-memory/export", Value::Null).await;
        bundle["memories"] = serde_json::json!([row.clone()]);
        let (status, error) = post_as(
            &router,
            &other_token,
            "/cairn-memory/import",
            serde_json::json!({"bundle": bundle}),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(
            error
                .to_string()
                .contains("account does not own the project")
        );
        assert!(!error.to_string().contains("projhttp"));
        assert_eq!(std::fs::read(&path).unwrap(), before);

        // The owner imports the same shape into its own project.
        let (_, mut own) =
            post_as(&router, &owner_token, "/cairn-memory/export", Value::Null).await;
        own["memories"] = serde_json::json!([row]);
        let (status, imported) = post_as(
            &router,
            &owner_token,
            "/cairn-memory/import",
            serde_json::json!({"bundle": own}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(imported["imported_memories"], 1);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn account_forget_requires_an_owned_id_over_http() {
        let path = temp_store_path("forget-owned-http");
        let router = app(AppState::open(&path).unwrap());
        let password = "correct horse battery staple";
        // A legacy row written before any account exists (the unscoped, trusted path).
        let (_, legacy) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({"content":"forgethttp legacy local note"}),
        )
        .await;
        let legacy_id = legacy["memory_id"].as_str().unwrap().to_owned();
        let (_, owner) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({"email":"owner@example.test","password":password}),
        )
        .await;
        let (_, other) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/register",
            serde_json::json!({"email":"other@example.test","password":password}),
        )
        .await;
        let owner_token = owner["token"].as_str().unwrap().to_owned();
        let other_token = other["token"].as_str().unwrap().to_owned();
        let (_, mine) = post_as(
            &router,
            &owner_token,
            "/cairn-memory/remember",
            serde_json::json!({"content":"forgethttp owner note"}),
        )
        .await;
        let mine_id = mine["memory_id"].as_str().unwrap().to_owned();
        let before = std::fs::read(&path).unwrap();
        for id in [legacy_id.as_str(), mine_id.as_str(), "no-such-id"] {
            let (status, error) = post_as(
                &router,
                &other_token,
                "/cairn-memory/forget",
                serde_json::json!({"id": id}),
            )
            .await;
            assert_ne!(status, StatusCode::NO_CONTENT);
            assert!(error.to_string().contains("unknown memory or event"));
            assert!(!error.to_string().contains("forgethttp"));
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let (status, _) = post_as(
            &router,
            &owner_token,
            "/cairn-memory/forget",
            serde_json::json!({"id": mine_id}),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn project_maintenance_is_owner_only_over_http() {
        let path = temp_store_path("maintenance-http");
        let router = app(AppState::open(&path).unwrap());
        let password = "correct horse battery staple";
        let (_, owner) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({"email":"owner@example.test","password":password}),
        )
        .await;
        let (_, other) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/register",
            serde_json::json!({"email":"other@example.test","password":password}),
        )
        .await;
        let owner_token = owner["token"].as_str().unwrap().to_owned();
        let other_token = other["token"].as_str().unwrap().to_owned();
        let (_, project) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/projects",
            serde_json::json!({"repository":"cairn-demo","name":"Demo"}),
            &owner_token,
        )
        .await;
        let project_id = project["project_id"].as_str().unwrap().to_owned();
        let url = format!("/cairn-memory/projects/{project_id}/maintenance");
        let (status, error) = post_as(
            &router,
            &other_token,
            &url,
            serde_json::json!({"currentCommit":"abc"}),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(
            error
                .to_string()
                .contains("account does not own the project")
        );
        assert!(error.get("active_memory_ids").is_none());
        let (status, report) = post_as(&router, &owner_token, &url, serde_json::json!({})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(report["project_id"], project_id);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn project_registry_is_owner_only_over_http() {
        let path = temp_store_path("registry-http");
        let router = app(AppState::open(&path).unwrap());
        let password = "correct horse battery staple";
        let (_, owner) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({"email":"owner@example.test","password":password}),
        )
        .await;
        let (_, other) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/register",
            serde_json::json!({"email":"other@example.test","password":password}),
        )
        .await;
        let owner_token = owner["token"].as_str().unwrap().to_owned();
        let other_token = other["token"].as_str().unwrap().to_owned();
        let (_, project) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/projects",
            serde_json::json!({"repository":"cairn-demo","name":"Demo"}),
            &owner_token,
        )
        .await;
        let project_id = project["project_id"].as_str().unwrap().to_owned();
        let url = format!("/cairn-memory/projects/{project_id}/branches");
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &url,
            serde_json::json!({"name":"feature/owner"}),
            &owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let before = std::fs::read(&path).unwrap();
        // An outsider who knows the project id can neither read nor write the registry.
        let (status, error) =
            json_request_auth(router.clone(), "GET", &url, Value::Null, &other_token).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(
            error
                .to_string()
                .contains("account does not own the project")
        );
        assert!(!error.to_string().contains("feature/owner"));
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &url,
            serde_json::json!({"name":"feature/evil"}),
            &other_token,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let (status, branches) =
            json_request_auth(router.clone(), "GET", &url, Value::Null, &owner_token).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(branches.as_array().unwrap().len(), 1);
        let _ = std::fs::remove_file(path);
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
        let (status, member_projects) = json_request_auth(
            router.clone(),
            "GET",
            "/cairn-memory/projects",
            Value::Null,
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            member_projects
                .as_array()
                .unwrap()
                .iter()
                .any(|project| project["project_id"] == project_id)
        );

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
        let (status, exported) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/export",
            serde_json::json!({}),
            owner_token,
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

    #[tokio::test]
    async fn layer5_policy_candidates_conflicts_and_private_reads() {
        let path = temp_store_path("project-share-policy");
        let router = app(AppState::open(&path).unwrap());
        let password = "correct horse battery staple";
        let (_, owner) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({
                "email": "policy-owner@example.test",
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
                "email": "policy-member@example.test",
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
                "name": "Policy Demo",
                "defaultBranch": "main"
            }),
            owner_token,
        )
        .await;
        let project_id = project["project_id"].as_str().unwrap();
        let members = format!("/cairn-memory/projects/{project_id}/members");
        let shares = format!("/cairn-memory/projects/{project_id}/shares");
        let shared = format!("/cairn-memory/projects/{project_id}/shared");
        let candidates = format!(
            "/cairn-memory/projects/{project_id}/share-candidates?email=policy-member%40example.test"
        );
        let policy = format!("/cairn-memory/projects/{project_id}/policy");
        let conflicts = format!("/cairn-memory/projects/{project_id}/conflicts");
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &members,
            serde_json::json!({"email": "policy-member@example.test"}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (_, manual_source) = json_request_auth(
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
        let manual_source_id = manual_source["memory_id"].as_str().unwrap();
        let (_, manual_promoted) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/promote",
            serde_json::json!({
                "projectId": project_id,
                "memoryId": manual_source_id,
                "reason": "Manual policy keeps this decision unshared"
            }),
            owner_token,
        )
        .await;
        let manual_id = manual_promoted["memory_id"].as_str().unwrap();
        let (status, before) =
            json_request_auth(router.clone(), "GET", &shared, Value::Null, member_token).await;
        assert_eq!(status, StatusCode::OK);
        assert!(before.as_array().unwrap().is_empty());
        let (status, listed) =
            json_request_auth(router.clone(), "GET", &candidates, Value::Null, owner_token).await;
        assert_eq!(status, StatusCode::OK);
        let candidate = listed
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["memory_id"] == manual_id)
            .unwrap();
        assert!(candidate["reason"].as_str().unwrap().contains("not shared"));
        assert!(
            candidate["not_auto_shared_reason"]
                .as_str()
                .unwrap()
                .contains("manual")
        );
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &shares,
            serde_json::json!({
                "memoryId": manual_id,
                "email": "policy-member@example.test"
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, accepted) =
            json_request_auth(router.clone(), "GET", &shared, Value::Null, member_token).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(accepted[0]["memory_id"], manual_id);
        assert_eq!(accepted[0]["share_reason"], "manual");
        assert!(accepted[0]["promoted_from"].is_null());
        assert!(!accepted.to_string().contains(manual_source_id));

        let (_, secret) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "api_key=super-secret-value",
                "context": {"repository": "cairn-demo"}
            }),
            owner_token,
        )
        .await;
        let secret_id = secret["memory_id"].as_str().unwrap();
        let (status, secret_candidates) =
            json_request_auth(router.clone(), "GET", &candidates, Value::Null, owner_token).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            secret_candidates
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["memory_id"] != secret_id)
        );
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &shares,
            serde_json::json!({
                "memoryId": secret_id,
                "email": "policy-member@example.test"
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &policy,
            serde_json::json!({"policy": "auto_nonsensitive_project_memory"}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, auto_source) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "nonsensitive repository decision is ready for collaborators",
                "context": {"repository": "cairn-demo", "commit": "def5678"}
            }),
            owner_token,
        )
        .await;
        let auto_source_id = auto_source["memory_id"].as_str().unwrap();
        let (_, auto_promoted) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/promote",
            serde_json::json!({
                "projectId": project_id,
                "memoryId": auto_source_id,
                "reason": "Automatic policy shares this nonsensitive decision"
            }),
            owner_token,
        )
        .await;
        let auto_id = auto_promoted["memory_id"].as_str().unwrap();
        let (_, sensitive_source) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "confidential repository note stays unshared until review",
                "context": {"repository": "cairn-demo"}
            }),
            owner_token,
        )
        .await;
        let (_, sensitive_promoted) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/promote",
            serde_json::json!({
                "projectId": project_id,
                "memoryId": sensitive_source["memory_id"],
                "reason": "Sensitive project memory stays a candidate"
            }),
            owner_token,
        )
        .await;
        let sensitive_id = sensitive_promoted["memory_id"].as_str().unwrap();
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
        let (status, after_auto) =
            json_request_auth(router.clone(), "GET", &shared, Value::Null, member_token).await;
        assert_eq!(status, StatusCode::OK);
        let shared_ids: Vec<_> = after_auto
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["memory_id"].as_str().unwrap())
            .collect();
        assert!(shared_ids.contains(&auto_id));
        assert!(!shared_ids.contains(&sensitive_id));
        assert!(!shared_ids.contains(&private_id));
        let auto_read = after_auto
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["memory_id"] == auto_id)
            .unwrap();
        assert_eq!(
            auto_read["share_reason"],
            "auto_nonsensitive_project_memory"
        );
        assert!(auto_read["promoted_from"].is_null());
        assert!(!auto_read.to_string().contains(auto_source_id));
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

        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &policy,
            serde_json::json!({"policy": "manual"}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, still_shared) =
            json_request_auth(router.clone(), "GET", &shared, Value::Null, member_token).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            still_shared
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["memory_id"] == auto_id)
        );

        let (_, dismissed_source) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "repository handoff note waits for an owner decision",
                "context": {"repository": "cairn-demo"}
            }),
            owner_token,
        )
        .await;
        let (_, dismissed_promoted) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/promote",
            serde_json::json!({
                "projectId": project_id,
                "memoryId": dismissed_source["memory_id"],
                "reason": "Dismiss this candidate instead of sharing it"
            }),
            owner_token,
        )
        .await;
        let dismissed_id = dismissed_promoted["memory_id"].as_str().unwrap();
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/share-candidates/{dismissed_id}/dismiss"),
            serde_json::json!({"email": "policy-member@example.test"}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, after_dismiss) =
            json_request_auth(router.clone(), "GET", &candidates, Value::Null, owner_token).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            after_dismiss
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["memory_id"] != dismissed_id)
        );
        let (status, after_dismiss_shared) =
            json_request_auth(router.clone(), "GET", &shared, Value::Null, member_token).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            after_dismiss_shared
                .as_array()
                .unwrap()
                .iter()
                .all(|item| item["memory_id"] != dismissed_id)
        );

        let (_, first_source) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "The project uses Redis as the default cache because reads are fast.",
                "context": {"repository": "cairn-demo"}
            }),
            owner_token,
        )
        .await;
        let (_, second_source) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": "The project does not use Redis as the default cache because consistency is preferred.",
                "context": {"repository": "cairn-demo"}
            }),
            owner_token,
        )
        .await;
        let (_, first_promoted) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/promote",
            serde_json::json!({
                "projectId": project_id,
                "memoryId": first_source["memory_id"],
                "reason": "Keep the first cache claim"
            }),
            owner_token,
        )
        .await;
        let (_, second_promoted) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/promote",
            serde_json::json!({
                "projectId": project_id,
                "memoryId": second_source["memory_id"],
                "reason": "Keep the contradictory cache claim"
            }),
            owner_token,
        )
        .await;
        let first_id = first_promoted["memory_id"].as_str().unwrap();
        let second_id = second_promoted["memory_id"].as_str().unwrap();
        let (status, conflict_list) =
            json_request_auth(router.clone(), "GET", &conflicts, Value::Null, owner_token).await;
        assert_eq!(status, StatusCode::OK);
        assert!(conflict_list.as_array().unwrap().iter().any(|conflict| {
            let ids = conflict["memory_ids"].as_array().unwrap();
            ids.iter().any(|id| id == first_id) && ids.iter().any(|id| id == second_id)
        }));
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
                .any(|memory| memory["memory_id"] == first_id)
        );
        assert!(
            owner_memories
                .as_array()
                .unwrap()
                .iter()
                .any(|memory| memory["memory_id"] == second_id)
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
        assert!(audit.as_array().unwrap().iter().any(|event| {
            event["action"] == "project_memory_shared" && event["metadata"]["memory_id"] == auto_id
        }));
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn layer5_team_space_permissions_and_retained_delete() {
        let path = temp_store_path("project-team-delete");
        let router = app(AppState::open(&path).unwrap());
        let password = "correct horse battery staple";
        let decision = "retained delete keeps this project decision stored";
        let (_, owner) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({
                "email": "team-owner@example.test",
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
                "email": "team-member@example.test",
                "password": password
            }),
        )
        .await;
        let member_token = member["token"].as_str().unwrap();
        let (_, stranger) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/register",
            serde_json::json!({
                "email": "team-stranger@example.test",
                "password": password
            }),
        )
        .await;
        let stranger_token = stranger["token"].as_str().unwrap();
        let (_, project) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/projects",
            serde_json::json!({
                "repository": "cairn-demo",
                "name": "Team Demo",
                "defaultBranch": "main"
            }),
            owner_token,
        )
        .await;
        let project_id = project["project_id"].as_str().unwrap();
        let (_, source) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({
                "content": decision,
                "context": {"repository": "cairn-demo", "commit": "abc1234"}
            }),
            owner_token,
        )
        .await;
        let (_, promoted) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/promote",
            serde_json::json!({
                "projectId": project_id,
                "memoryId": source["memory_id"],
                "reason": "Share the retained decision with the team"
            }),
            owner_token,
        )
        .await;
        let promoted_id = promoted["memory_id"].as_str().unwrap();
        let team = format!("/cairn-memory/projects/{project_id}/team");
        let team_members = format!("/cairn-memory/projects/{project_id}/team/members");
        let team_remove = format!("/cairn-memory/projects/{project_id}/team/members/remove");
        let shares = format!("/cairn-memory/projects/{project_id}/shares");
        let shared = format!("/cairn-memory/projects/{project_id}/shared");
        let revoke = format!("/cairn-memory/projects/{project_id}/shares/{promoted_id}/revoke");

        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &format!("/cairn-memory/projects/{project_id}/members"),
            serde_json::json!({"email": "team-member@example.test"}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &shares,
            serde_json::json!({
                "memoryId": promoted_id,
                "email": "team-member@example.test"
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, stranger_team) =
            json_request_auth(router.clone(), "GET", &team, Value::Null, stranger_token).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            stranger_team["error"]
                .as_str()
                .unwrap()
                .contains("not a project member")
        );
        let (status, stranger_shared) =
            json_request_auth(router.clone(), "GET", &shared, Value::Null, stranger_token).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            stranger_shared["error"]
                .as_str()
                .unwrap()
                .contains("not a project member")
        );

        let (status, member_create) = json_request_auth(
            router.clone(),
            "POST",
            &team,
            serde_json::json!({"name": "Platform"}),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(
            member_create["error"]
                .as_str()
                .unwrap()
                .contains("does not own")
        );
        let (status, created) = json_request_auth(
            router.clone(),
            "POST",
            &team,
            serde_json::json!({"name": "Platform"}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(created["name"], "Platform");
        let (status, added) = json_request_auth(
            router.clone(),
            "POST",
            &team_members,
            serde_json::json!({"email": "team-member@example.test"}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(added["email"], "team-member@example.test");
        let (status, listed) =
            json_request_auth(router.clone(), "GET", &team, Value::Null, member_token).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(listed["members"][0]["email"], "team-member@example.test");
        let (status, _) =
            json_request_auth(router.clone(), "GET", &team, Value::Null, stranger_token).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, member_share) = json_request_auth(
            router.clone(),
            "POST",
            &shares,
            serde_json::json!({
                "memoryId": promoted_id,
                "email": "team-member@example.test"
            }),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(
            member_share["error"]
                .as_str()
                .unwrap()
                .contains("does not own")
        );
        let (status, member_revoke) = json_request_auth(
            router.clone(),
            "POST",
            &revoke,
            serde_json::json!({"email": "team-member@example.test"}),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(
            member_revoke["error"]
                .as_str()
                .unwrap()
                .contains("does not own")
        );
        let (status, member_correct) = json_request_auth(
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
            member_correct["error"]
                .as_str()
                .unwrap()
                .contains("does not own")
        );
        let (status, member_forget) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/forget",
            serde_json::json!({"id": promoted_id}),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(
            member_forget["error"]
                .as_str()
                .unwrap()
                .contains("does not own")
        );
        let (status, member_delete) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/delete",
            serde_json::json!({"id": promoted_id}),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(
            member_delete["error"]
                .as_str()
                .unwrap()
                .contains("does not own")
        );

        let (_, note) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({"content": "owner correction source stays stored"}),
            owner_token,
        )
        .await;
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/correct",
            serde_json::json!({
                "targetId": note["memory_id"],
                "content": "The owner correction replaces the note."
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/forget",
            serde_json::json!({"id": note["memory_id"]}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &revoke,
            serde_json::json!({"email": "team-member@example.test"}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &shares,
            serde_json::json!({
                "memoryId": promoted_id,
                "email": "team-member@example.test"
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            &team_remove,
            serde_json::json!({"email": "team-member@example.test"}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, former_shared) =
            json_request_auth(router.clone(), "GET", &shared, Value::Null, member_token).await;
        assert_eq!(status, StatusCode::OK);
        assert!(former_shared.as_array().unwrap().is_empty());
        let (status, exported) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/export",
            serde_json::json!({}),
            owner_token,
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

        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/delete",
            serde_json::json!({"id": promoted_id}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        let (status, memories) = json_request_auth(
            router.clone(),
            "GET",
            &format!("/cairn-memory/projects/{project_id}/memories"),
            Value::Null,
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            memories
                .as_array()
                .unwrap()
                .iter()
                .all(|memory| memory["memory_id"] != promoted_id)
        );
        let (status, found) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/search",
            serde_json::json!({"query": "retained delete keeps", "limit": 5}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            found
                .as_array()
                .unwrap()
                .iter()
                .all(|memory| memory["memory_id"] != promoted_id)
        );
        let (status, after_delete) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/export",
            serde_json::json!({}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            after_delete["memories"]
                .as_array()
                .unwrap()
                .iter()
                .any(|memory| memory["memory_id"] == promoted_id)
        );
        assert!(
            after_delete["forgottenIds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == promoted_id)
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
        for action in [
            "project_team_created",
            "project_team_member_added",
            "project_team_member_removed",
            "memory_promote",
            "memory_forget",
            "memory_delete_retained",
        ] {
            assert!(actions.contains(&action), "{action}");
        }
        let audit_text = audit.to_string();
        assert!(!audit_text.contains(decision));
        assert!(!audit_text.contains(password));
        assert!(!audit_text.contains(owner_token));
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn member_without_authorization_is_rejected_and_cannot_export_a_private_observation() {
        let path = temp_store_path("file-auth");
        let router = app(AppState::open(&path).unwrap());
        let password = "correct horse battery staple";
        let (_, owner) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({
                "email": "owner@example.test",
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
                "email": "member@example.test",
                "password": password
            }),
        )
        .await;
        let member_token = member["token"].as_str().unwrap();
        let (status, captured) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/observe",
            serde_json::json!({
                "eventId": "owner-private-evt",
                "sessionId": "owner-private-session",
                "content": "owner private observation",
                "role": "agent",
                "source": "test"
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(captured["observation"]["event_id"], "owner-private-evt");

        let (status, _) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/export",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, member_export) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/export",
            serde_json::json!({}),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            member_export["observations"]
                .as_array()
                .unwrap()
                .iter()
                .all(|observation| observation["event_id"] != "owner-private-evt")
        );

        let (status, owner_export) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/export",
            serde_json::json!({}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            owner_export["observations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|observation| observation["content"] == "owner private observation")
        );
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn file_diagnose_needs_the_account_bearer_and_counts_owned_rows() {
        let path = temp_store_path("file-diagnose-auth");
        let router = app(AppState::open(&path).unwrap());
        let password = "correct horse battery staple";
        let mut tokens = Vec::new();
        for (route, email) in [
            ("bootstrap", "diagnose-owner@example.test"),
            ("register", "diagnose-member@example.test"),
        ] {
            let (_, account) = json_request(
                router.clone(),
                "POST",
                &format!("/cairn-memory/auth/{route}"),
                serde_json::json!({"email": email, "password": password}),
            )
            .await;
            tokens.push(account["token"].as_str().unwrap().to_owned());
        }
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({"content": "owner durable note"}),
            &tokens[0],
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) =
            json_request(router.clone(), "GET", "/cairn-memory/diagnose", Value::Null).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, owner) = json_request_auth(
            router.clone(),
            "GET",
            "/cairn-memory/diagnose",
            Value::Null,
            &tokens[0],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(owner["durable_memories"], 1);
        let (status, member) = json_request_auth(
            router,
            "GET",
            "/cairn-memory/diagnose",
            Value::Null,
            &tokens[1],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(member["durable_memories"], 0);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn file_http_capture_uses_file_bearer_when_accounts_exist() {
        let path = temp_store_path("file-capture-auth");
        let router = app(AppState::open(&path).unwrap());
        let (status, _) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/session/start",
            serde_json::json!({"sessionId": "anon-session", "context": {}}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, anon) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/observe",
            serde_json::json!({
                "eventId": "anon-event",
                "sessionId": "anon-session",
                "content": "anonymousstoredmarker",
                "role": "agent",
                "source": "hook"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(anon["duplicate"], false);

        let password = "correct horse battery staple";
        let (_, owner) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/bootstrap",
            serde_json::json!({
                "email": "capture-owner@example.test",
                "password": password
            }),
        )
        .await;
        let owner_token = owner["token"].as_str().unwrap();
        let owner_person = owner["account"]["person_id"].as_str().unwrap();
        let (_, member) = json_request(
            router.clone(),
            "POST",
            "/cairn-memory/auth/register",
            serde_json::json!({
                "email": "capture-member@example.test",
                "password": password
            }),
        )
        .await;
        let member_token = member["token"].as_str().unwrap();
        let member_person = member["account"]["person_id"].as_str().unwrap();
        let before = std::fs::read(&path).unwrap();
        let marker = "rejectedpayload";
        let rejected = [
            (
                "/cairn-memory/session/start",
                serde_json::json!({
                    "sessionId": "rejected-session",
                    "context": {"branch": "rejected-branch", "repository": marker}
                }),
            ),
            (
                "/cairn-memory/observe",
                serde_json::json!({
                    "eventId": "evt-owner",
                    "sessionId": "shared-session",
                    "content": marker,
                    "role": "agent",
                    "source": "test"
                }),
            ),
            (
                "/cairn-memory/session/end",
                serde_json::json!({"sessionId": "shared-session", "content": marker}),
            ),
            (
                "/cairn-memory/remember",
                serde_json::json!({"content": marker}),
            ),
        ];
        for token in [None, Some("not-a-session-token")] {
            for (route, body) in &rejected {
                let (status, value) = match token {
                    Some(token) => {
                        json_request_auth(router.clone(), "POST", route, body.clone(), token).await
                    }
                    None => json_request(router.clone(), "POST", route, body.clone()).await,
                };
                assert_eq!(status, StatusCode::UNAUTHORIZED, "{route} {value}");
                assert!(value.get("observation").is_none(), "{value}");
                assert!(value.get("memory_id").is_none(), "{value}");
                assert!(value.get("content").is_none(), "{value}");
                let error = value.to_string();
                assert!(!error.contains(marker), "{error}");
                assert!(!error.contains("anonymousstoredmarker"), "{error}");
            }
        }
        let after = std::fs::read(&path).unwrap();
        assert!(before == after, "rejected capture changed the store file");

        let (status, anon_account) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/session/start",
            serde_json::json!({
                "sessionId": "anon-session",
                "context": {"repository": "account-repo", "branch": "account-branch"}
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(anon_account["person_id"], owner_person);
        assert_eq!(anon_account["context"]["branch"], "account-branch");
        let (status, anon_missing) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/session/end",
            serde_json::json!({"sessionId": "anon-session"}),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(anon_missing.to_string().contains("unknown session"));
        assert!(!anon_missing.to_string().contains("account-branch"));

        let (status, started) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/session/start",
            serde_json::json!({
                "sessionId": "shared-session",
                "context": {
                    "repository": "owner-repo",
                    "branch": "owner-branch",
                    "worktree": "owner-tree"
                }
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(started["context"]["branch"], "owner-branch");
        assert_eq!(started["person_id"], owner_person);
        let (status, owner_retry) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/session/start",
            serde_json::json!({
                "sessionId": "shared-session",
                "context": {"repository": "member-repo", "branch": "member-branch"}
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(owner_retry["context"]["branch"], "owner-branch");
        assert_eq!(owner_retry["context"]["worktree"], "owner-tree");
        let (status, member_started) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/session/start",
            serde_json::json!({
                "sessionId": "shared-session",
                "context": {"repository": "member-repo", "branch": "member-branch"}
            }),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(member_started["person_id"], member_person);
        assert_eq!(member_started["context"]["branch"], "member-branch");
        assert_eq!(member_started["context"]["repository"], "member-repo");
        assert!(member_started["context"]["worktree"].is_null());

        let (status, captured) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/observe",
            serde_json::json!({
                "eventId": "evt-owner",
                "sessionId": "shared-session",
                "content": "owneronlymarker",
                "role": "agent",
                "source": "test"
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(captured["duplicate"], false);
        assert_eq!(captured["observation"]["content"], "owneronlymarker");
        assert_eq!(captured["observation"]["context"]["branch"], "owner-branch");
        assert_eq!(captured["observation"]["context"]["worktree"], "owner-tree");
        assert_ne!(
            captured["observation"]["context"]["branch"],
            "member-branch"
        );
        let (status, retry) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/observe",
            serde_json::json!({
                "eventId": "evt-owner",
                "sessionId": "shared-session",
                "content": "ownerretrytext",
                "role": "agent",
                "source": "test"
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(retry.get("observation").is_none(), "{retry}");
        let (status, retry) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/observe",
            serde_json::json!({
                "eventId": "evt-owner",
                "sessionId": "shared-session",
                "content": "owneronlymarker",
                "role": "agent",
                "source": "test"
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(retry["duplicate"], true);
        assert_eq!(retry["observation"]["content"], "owneronlymarker");

        let (status, foreign) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/observe",
            serde_json::json!({
                "eventId": "evt-owner",
                "sessionId": "shared-session",
                "content": "memberstealtext",
                "role": "agent",
                "source": "test"
            }),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let foreign_body = foreign.to_string();
        assert!(foreign_body.contains("another person"));
        assert!(foreign.get("observation").is_none(), "{foreign}");
        for secret in [
            "owneronlymarker",
            "memberstealtext",
            "anonymousstoredmarker",
            marker,
        ] {
            assert!(!foreign_body.contains(secret), "{foreign_body}");
        }

        let (status, unowned) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/observe",
            serde_json::json!({
                "eventId": "anon-event",
                "sessionId": "anon-session",
                "content": "claimattemptmarker",
                "role": "agent",
                "source": "test"
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        let unowned_body = unowned.to_string();
        assert!(unowned_body.contains("outside sync scope"));
        assert!(unowned.get("observation").is_none(), "{unowned}");
        for secret in ["anonymousstoredmarker", "claimattemptmarker", marker] {
            assert!(!unowned_body.contains(secret), "{unowned_body}");
        }

        let (status, member_capture) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/observe",
            serde_json::json!({
                "eventId": "evt-member",
                "sessionId": "shared-session",
                "content": "memberonlymarker",
                "role": "agent",
                "source": "test"
            }),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(member_capture["duplicate"], false);
        assert_eq!(
            member_capture["observation"]["context"]["branch"],
            "member-branch"
        );
        assert_ne!(
            member_capture["observation"]["context"]["branch"],
            "owner-branch"
        );
        let (status, member_sent) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/observe",
            serde_json::json!({
                "eventId": "evt-member-sent",
                "sessionId": "shared-session",
                "content": "membersentmarker",
                "role": "agent",
                "source": "test",
                "context": {"files": ["member-file.rs"]}
            }),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            member_sent["observation"]["context"]["files"][0],
            "member-file.rs"
        );
        assert!(member_sent["observation"]["context"]["branch"].is_null());

        let (status, owner_end) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/session/end",
            serde_json::json!({"sessionId": "shared-session"}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            owner_end["memory_id"],
            format!("memory-{owner_person}-shared-session")
        );
        let owner_summary = owner_end["content"].as_str().unwrap();
        assert!(owner_summary.contains("owneronlymarker"));
        assert!(!owner_summary.contains("memberonlymarker"));
        assert!(!owner_summary.contains("membersentmarker"));
        assert!(!owner_summary.contains("anonymousstoredmarker"));
        assert_eq!(owner_end["scope"], "worktree");
        assert_eq!(owner_end["context"]["branch"], "owner-branch");
        let owner_events = owner_end["source_event_ids"].as_array().unwrap();
        assert!(owner_events.iter().any(|id| id == "evt-owner"));
        assert!(owner_events.iter().all(|id| id != "evt-member"));

        let (status, member_end) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/session/end",
            serde_json::json!({"sessionId": "shared-session"}),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            member_end["memory_id"],
            format!("memory-{member_person}-shared-session")
        );
        assert_ne!(member_end["memory_id"], owner_end["memory_id"]);
        let member_summary = member_end["content"].as_str().unwrap();
        assert!(member_summary.contains("memberonlymarker"));
        assert!(!member_summary.contains("owneronlymarker"));
        assert_eq!(member_end["scope"], "branch");
        assert_eq!(member_end["context"]["repository"], "member-repo");
        assert_ne!(member_end["context"]["branch"], "owner-branch");
        let member_events = member_end["source_event_ids"].as_array().unwrap();
        assert!(member_events.iter().any(|id| id == "evt-member"));
        assert!(member_events.iter().all(|id| id != "evt-owner"));

        let (status, owner_hits) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/search",
            serde_json::json!({"query": "owneronlymarker", "limit": 10}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(owner_hits.as_array().unwrap().len(), 1);
        let (status, owner_miss) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/search",
            serde_json::json!({"query": "memberonlymarker", "limit": 10}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(owner_miss.as_array().unwrap().is_empty());
        let (status, member_hits) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/search",
            serde_json::json!({"query": "memberonlymarker", "limit": 10}),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(member_hits.as_array().unwrap().len(), 1);
        let (status, member_miss) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/search",
            serde_json::json!({"query": "owneronlymarker", "limit": 10}),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(member_miss.as_array().unwrap().is_empty());

        let (status, owner_memory) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({"content": "ownerremembermarker"}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            owner_memory["memory_id"]
                .as_str()
                .unwrap()
                .starts_with("memory-")
        );
        let (status, member_memory) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/remember",
            serde_json::json!({"content": "memberremembermarker"}),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, owner_remember) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/search",
            serde_json::json!({"query": "ownerremembermarker", "limit": 10}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(owner_remember.as_array().unwrap().len(), 1);
        assert_eq!(owner_remember[0]["memory_id"], owner_memory["memory_id"]);
        let (status, owner_remember_miss) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/search",
            serde_json::json!({"query": "memberremembermarker", "limit": 10}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(owner_remember_miss.as_array().unwrap().is_empty());
        let (status, member_remember) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/search",
            serde_json::json!({"query": "memberremembermarker", "limit": 10}),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(member_remember.as_array().unwrap().len(), 1);
        assert_eq!(member_remember[0]["memory_id"], member_memory["memory_id"]);
        let (status, member_remember_miss) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/search",
            serde_json::json!({"query": "ownerremembermarker", "limit": 10}),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(member_remember_miss.as_array().unwrap().is_empty());

        let (status, owner_export) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/export",
            serde_json::json!({}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let owner_sessions = owner_export["sessions"].as_array().unwrap();
        assert!(
            owner_sessions
                .iter()
                .all(|session| session["person_id"] == owner_person)
        );
        assert!(
            owner_sessions
                .iter()
                .any(|session| session["context"]["branch"] == "owner-branch")
        );
        assert!(
            owner_sessions
                .iter()
                .all(|session| session["context"]["branch"] != "member-branch")
        );
        let (status, member_export) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/export",
            serde_json::json!({}),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let member_sessions = member_export["sessions"].as_array().unwrap();
        assert!(
            member_sessions
                .iter()
                .all(|session| session["person_id"] == member_person)
        );
        assert!(
            member_sessions
                .iter()
                .all(|session| session["context"]["branch"] != "owner-branch")
        );
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw["version"], 1);

        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/session/start",
            serde_json::json!({"sessionId": "handoff-topic", "context": {}}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/session/start",
            serde_json::json!({"sessionId": "handoff-topic", "context": {}}),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/observe",
            serde_json::json!({
                "eventId": "owner-handoff-evt",
                "sessionId": "handoff-topic",
                "content": "quotadelta widget does not apply remaining",
                "role": "agent",
                "source": "test"
            }),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/observe",
            serde_json::json!({
                "eventId": "member-handoff-evt",
                "sessionId": "handoff-topic",
                "content": "quotadelta widget does apply foreignmarker remaining",
                "role": "agent",
                "source": "test"
            }),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/session/end",
            serde_json::json!({"sessionId": "handoff-topic"}),
            owner_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/session/end",
            serde_json::json!({"sessionId": "handoff-topic"}),
            member_token,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let foreign_memory = format!("memory-{member_person}-handoff-topic");
        for route in ["/cairn-memory/resume", "/cairn-memory/unresolved"] {
            let (status, pack) = json_request_auth(
                router.clone(),
                "POST",
                route,
                serde_json::json!({"limit": 5}),
                owner_token,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{route}");
            let body = pack.to_string();
            assert!(!body.contains(member_person), "{route} {body}");
            assert!(!body.contains(&foreign_memory), "{route} {body}");
            assert!(!body.contains("member-handoff-evt"), "{route} {body}");
            assert!(!body.contains("foreignmarker"), "{route} {body}");
            let handoff = pack["results"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| {
                    row["memory_id"]
                        .as_str()
                        .unwrap()
                        .ends_with("-handoff-topic")
                })
                .unwrap_or_else(|| panic!("{route} missing handoff"));
            assert_eq!(handoff["contradictory"], false, "{route}");
            assert!(handoff["contradiction_ids"].as_array().unwrap().is_empty());
            let (status, zero) = json_request_auth(
                router.clone(),
                "POST",
                route,
                serde_json::json!({"limit": 0}),
                owner_token,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{route}");
            assert!(zero["results"].as_array().unwrap().is_empty(), "{route}");
            let zero_body = zero.to_string();
            assert!(!zero_body.contains(&foreign_memory), "{route} {zero_body}");
            assert!(!zero_body.contains("foreignmarker"), "{route} {zero_body}");
        }
        let anon_unscoped = raw["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|session| {
                session["session_id"] == "anon-session" && session.get("person_id").is_none()
            })
            .expect("unscoped anon session");
        assert!(anon_unscoped["ended_at"].is_null());
        assert!(anon_unscoped["context"]["branch"].is_null());
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn file_device_errors_match_postgres_statuses() {
        let path = temp_store_path("device-error-statuses");
        let router = app(AppState::open(&path).unwrap());
        let password = "correct horse battery staple";
        let mut tokens = Vec::new();
        for (route, email) in [
            ("bootstrap", "dev-a@example.test"),
            ("register", "dev-b@example.test"),
        ] {
            let (_, account) = json_request(
                router.clone(),
                "POST",
                &format!("/cairn-memory/auth/{route}"),
                serde_json::json!({"email": email, "password": password}),
            )
            .await;
            let token = account["token"].as_str().unwrap().to_owned();
            let person = account["account"]["person_id"].as_str().unwrap().to_owned();
            let device_id = format!("device-{route}");
            let (status, device) = json_request_auth(
                router.clone(),
                "POST",
                "/cairn-memory/devices/register",
                serde_json::json!({"deviceId": device_id, "personId": person, "name": route}),
                &token,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            tokens.push((
                token,
                person,
                device_id,
                device["token"].as_str().unwrap().to_owned(),
            ));
        }

        // An account acting on another person's device is out of scope: 401, as on PostgreSQL.
        for action in ["revoke", "rotate"] {
            let (status, body) = json_request_auth(
                router.clone(),
                "POST",
                &format!("/cairn-memory/devices/{action}"),
                serde_json::json!({"deviceId": tokens[1].2}),
                &tokens[0].0,
            )
            .await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{action}: {body}");
        }

        // A revoked device id cannot be registered again: 401, as on PostgreSQL.
        let (status, _) = json_request_auth(
            router.clone(),
            "POST",
            "/cairn-memory/devices/revoke",
            serde_json::json!({"deviceId": tokens[0].2}),
            &tokens[0].0,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, body) = json_request_auth(
            router,
            "POST",
            "/cairn-memory/devices/register",
            serde_json::json!({"deviceId": tokens[0].2, "personId": tokens[0].1, "name": "again"}),
            &tokens[0].0,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("device is revoked")
        );
        let _ = std::fs::remove_file(path);
    }
}
