//! Layer 1/2 personal memory and continuity primitives.
//!
//! This crate intentionally implements a small, local vertical slice:
//! capture -> redact -> persist -> summarize -> scoped recall -> forget. The
//! file store is a prototype persistence adapter for the local continuity
//! layers; the server-authoritative PostgreSQL adapter remains later.

use argon2::password_hash::{SaltString, rand_core::OsRng as PasswordRandomSource};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use rand::{RngCore, rngs::OsRng as TokenRandomSource};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const STORE_VERSION: u32 = 1;
pub const EXPORT_FORMAT: &str = "cairn-memory";
pub const EXPORT_VERSION: u32 = 1;
const REDACTED: &str = "[REDACTED]";
const MAX_OBSERVATION_BYTES: usize = 64 * 1024;
const MAX_MEMORY_BYTES: usize = 256 * 1024;
const MAX_HANDOFF_ITEMS: usize = 16;
const MAX_HANDOFF_ITEM_BYTES: usize = 2048;
const MAX_RELATIONSHIPS: usize = 32;
const DEFAULT_TEMPORARY_RETENTION_SECONDS: u64 = 30 * 24 * 60 * 60;
pub const AUTH_SESSION_SECONDS: u64 = 30 * 24 * 60 * 60;
const MIN_PASSWORD_BYTES: usize = 12;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Public,
    #[default]
    Private,
    Sensitive,
    Secret,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Retention {
    #[default]
    Temporary,
    Durable,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    SessionSummary,
    #[default]
    Explicit,
    Lesson,
    Correction,
    Decision,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    #[default]
    Session,
    Worktree,
    Branch,
    Project,
    Global,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LifecycleInput {
    #[serde(default)]
    pub importance: Option<u8>,
    #[serde(default)]
    pub sensitivity: Option<Sensitivity>,
    #[serde(default)]
    pub confidence: Option<u8>,
    #[serde(default)]
    pub freshness: Option<u8>,
    #[serde(default)]
    pub retention: Option<Retention>,
    #[serde(rename = "expiresAt", default)]
    pub expires_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Lifecycle {
    pub importance: u8,
    pub sensitivity: Sensitivity,
    pub confidence: u8,
    pub freshness: u8,
    pub retention: Retention,
    #[serde(rename = "expiresAt", default)]
    pub expires_at: Option<String>,
}

impl Lifecycle {
    fn observation(input: &LifecycleInput, importance_hint: Option<&str>, content: &str) -> Self {
        let retention = input.retention.clone().unwrap_or_default();
        Self {
            importance: input
                .importance
                .or_else(|| importance_hint.map(importance_score))
                .unwrap_or(50),
            sensitivity: input
                .sensitivity
                .clone()
                .unwrap_or_else(|| classify_sensitivity(content)),
            confidence: input.confidence.unwrap_or(50),
            freshness: input.freshness.unwrap_or(100),
            retention: retention.clone(),
            expires_at: input.expires_at.clone().or_else(|| {
                (retention == Retention::Temporary)
                    .then(|| (now_seconds() + DEFAULT_TEMPORARY_RETENTION_SECONDS).to_string())
            }),
        }
    }

    fn durable_from(observations: &[Observation]) -> Self {
        let importance = observations
            .iter()
            .map(|observation| observation.lifecycle.importance)
            .max()
            .unwrap_or(50);
        let confidence = observations
            .iter()
            .map(|observation| observation.lifecycle.confidence as u16)
            .sum::<u16>()
            .checked_div(observations.len().max(1) as u16)
            .unwrap_or(50) as u8;
        let sensitivity = observations
            .iter()
            .map(|observation| observation.lifecycle.sensitivity.clone())
            .max_by_key(sensitivity_rank)
            .unwrap_or_default();
        Self {
            importance,
            sensitivity,
            confidence,
            freshness: 100,
            retention: Retention::Durable,
            expires_at: None,
        }
    }

    fn explicit(input: &LifecycleInput, content: &str) -> Self {
        let retention = input.retention.clone().unwrap_or(Retention::Durable);
        Self {
            importance: input.importance.unwrap_or(80),
            sensitivity: input
                .sensitivity
                .clone()
                .unwrap_or_else(|| classify_sensitivity(content)),
            confidence: input.confidence.unwrap_or(80),
            freshness: input.freshness.unwrap_or(100),
            retention: retention.clone(),
            expires_at: input.expires_at.clone().or_else(|| {
                (retention == Retention::Temporary)
                    .then(|| (now_seconds() + DEFAULT_TEMPORARY_RETENTION_SECONDS).to_string())
            }),
        }
    }
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            importance: 50,
            sensitivity: Sensitivity::Private,
            confidence: 50,
            freshness: 100,
            retention: Retention::Temporary,
            expires_at: None,
        }
    }
}

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    Json(serde_json::Error),
    Validation(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "storage error: {error}"),
            Self::Json(error) => write!(f, "store format error: {error}"),
            Self::Validation(message) => write!(f, "validation error: {message}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkContext {
    pub repository: Option<String>,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub commit: Option<String>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub symbols: Vec<String>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub tests: Vec<String>,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub handoffs: Vec<String>,
    #[serde(default)]
    pub runbooks: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObservationInput {
    pub event_id: String,
    pub session_id: String,
    pub content: String,
    pub role: String,
    pub source: String,
    pub importance: Option<String>,
    #[serde(default)]
    pub lifecycle: LifecycleInput,
    #[serde(default)]
    pub context: WorkContext,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Observation {
    pub event_id: String,
    pub session_id: String,
    pub content: String,
    pub role: String,
    pub source: String,
    pub importance: Option<String>,
    #[serde(default)]
    pub lifecycle: Lifecycle,
    #[serde(default)]
    pub scope: MemoryScope,
    pub context: WorkContext,
    pub captured_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub session_id: String,
    pub context: WorkContext,
    pub started_at: String,
    pub ended_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Memory {
    pub memory_id: String,
    pub session_id: String,
    pub content: String,
    pub source_event_ids: Vec<String>,
    pub context: WorkContext,
    #[serde(default)]
    pub lifecycle: Lifecycle,
    #[serde(default)]
    pub kind: MemoryKind,
    #[serde(default)]
    pub scope: MemoryScope,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub next_steps: Vec<String>,
    #[serde(default)]
    pub unresolved_work: Vec<String>,
    #[serde(default)]
    pub supersedes: Option<String>,
    #[serde(default)]
    pub promoted_from: Option<String>,
    #[serde(default)]
    pub promotion_reason: Option<String>,
    #[serde(default)]
    pub verified_commit: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RecallResult {
    pub memory_id: String,
    pub content: String,
    pub source_event_ids: Vec<String>,
    pub context: WorkContext,
    pub lifecycle: Lifecycle,
    pub scope: MemoryScope,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub next_steps: Vec<String>,
    #[serde(default)]
    pub unresolved_work: Vec<String>,
    #[serde(default)]
    pub decision_reason: Option<String>,
    #[serde(default)]
    pub relationships: Vec<CodeRelationship>,
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub stale_reason: Option<String>,
    #[serde(default)]
    pub contradictory: bool,
    #[serde(default)]
    pub contradiction_ids: Vec<String>,
    #[serde(default)]
    pub promoted_from: Option<String>,
    #[serde(default)]
    pub promotion_reason: Option<String>,
    #[serde(default)]
    pub verified_commit: Option<String>,
    pub score: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodeRelationship {
    pub relation: String,
    pub from: String,
    pub to: String,
    pub evidence: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ContextPack {
    pub query: String,
    pub results: Vec<RecallResult>,
    pub omitted_count: usize,
    #[serde(default)]
    pub omitted_memory_ids: Vec<String>,
    #[serde(default)]
    pub recovery_hint: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecallFilter {
    #[serde(rename = "sessionId", alias = "session_id", default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub context: Option<WorkContext>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExportBundle {
    pub format: String,
    pub version: u32,
    #[serde(rename = "exportedAt")]
    pub exported_at: String,
    pub sessions: Vec<Session>,
    pub observations: Vec<Observation>,
    pub memories: Vec<Memory>,
    #[serde(rename = "forgottenIds", default)]
    pub forgotten_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportResult {
    pub imported_sessions: usize,
    pub imported_observations: usize,
    pub imported_memories: usize,
    pub imported_forgotten_ids: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LifecycleSummary {
    pub temporary_observations: usize,
    pub durable_memories: usize,
    pub sensitive_items: usize,
    pub expired_items: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    #[default]
    Active,
    Disabled,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub person_id: String,
    pub email: String,
    #[serde(default)]
    pub status: AccountStatus,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthSessionResult {
    pub account: Account,
    pub token: String,
    #[serde(rename = "expiresAt")]
    pub expires_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoreState {
    version: u32,
    #[serde(default)]
    sessions: Vec<Session>,
    #[serde(default)]
    observations: Vec<Observation>,
    #[serde(default)]
    memories: Vec<Memory>,
    #[serde(default)]
    forgotten_ids: HashSet<String>,
    #[serde(default)]
    devices: Vec<DeviceRecord>,
    #[serde(default)]
    sync_events: Vec<SyncEvent>,
    #[serde(default)]
    next_sync_cursor: u64,
    #[serde(default)]
    accounts: Vec<AccountRecord>,
    #[serde(default)]
    auth_sessions: Vec<AuthSessionRecord>,
    #[serde(default)]
    projects: Vec<Project>,
    #[serde(default)]
    project_branches: Vec<ProjectBranch>,
    #[serde(default)]
    project_worktrees: Vec<ProjectWorktree>,
    #[serde(default)]
    project_entities: Vec<ProjectEntity>,
    #[serde(default)]
    freshness_events: Vec<ProjectFreshnessEvent>,
    #[serde(default)]
    project_owners: Vec<ProjectOwnerRecord>,
    #[serde(default)]
    memory_owners: Vec<MemoryOwnerRecord>,
    #[serde(default)]
    project_members: Vec<ProjectMemberRecord>,
    #[serde(default)]
    project_shares: Vec<ProjectShareRecord>,
    #[serde(default)]
    project_policies: Vec<ProjectPolicyRecord>,
    #[serde(default)]
    dismissed_share_candidates: Vec<DismissedShareCandidate>,
    #[serde(default)]
    project_teams: Vec<ProjectTeamRecord>,
    #[serde(default)]
    project_team_members: Vec<ProjectTeamMemberRecord>,
    #[serde(default)]
    audit_events: Vec<StoredAuditEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AccountRecord {
    account: Account,
    password_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AuthSessionRecord {
    session_id: String,
    person_id: String,
    token_hash: String,
    created_at: String,
    expires_at: String,
    revoked_at: Option<String>,
}

impl Default for StoreState {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            sessions: Vec::new(),
            observations: Vec::new(),
            memories: Vec::new(),
            forgotten_ids: HashSet::new(),
            devices: Vec::new(),
            sync_events: Vec::new(),
            next_sync_cursor: 0,
            accounts: Vec::new(),
            auth_sessions: Vec::new(),
            projects: Vec::new(),
            project_branches: Vec::new(),
            project_worktrees: Vec::new(),
            project_entities: Vec::new(),
            freshness_events: Vec::new(),
            project_owners: Vec::new(),
            memory_owners: Vec::new(),
            project_members: Vec::new(),
            project_shares: Vec::new(),
            project_policies: Vec::new(),
            dismissed_share_candidates: Vec::new(),
            project_teams: Vec::new(),
            project_team_members: Vec::new(),
            audit_events: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptureResult {
    pub accepted: bool,
    pub duplicate: bool,
    pub observation: Observation,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    #[default]
    Active,
    Revoked,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Device {
    pub device_id: String,
    pub person_id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub status: DeviceStatus,
    pub registered_at: String,
    pub last_seen_at: String,
    pub cursor: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceRegistration {
    pub device: Device,
    pub token: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncEvent {
    pub cursor: u64,
    pub event_id: String,
    pub person_id: String,
    pub device_id: String,
    pub observation: Observation,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncPushResult {
    pub accepted: bool,
    pub duplicate: bool,
    pub cursor: u64,
    pub observation: Observation,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncPullResult {
    pub cursor: u64,
    pub events: Vec<SyncEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncDiagnostics {
    pub device_id: String,
    pub device_cursor: u64,
    pub server_cursor: u64,
    pub lag: u64,
    pub pending_events: usize,
    pub rejected_events: usize,
    pub status: String,
    #[serde(default)]
    pub recovery_hint: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEvent {
    pub audit_id: String,
    pub action: String,
    pub subject_id: Option<String>,
    pub actor_device_id: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub project_id: String,
    pub repository: String,
    pub name: String,
    #[serde(default)]
    pub default_branch: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectBranch {
    pub branch_id: String,
    pub project_id: String,
    pub name: String,
    #[serde(default)]
    pub base_branch: Option<String>,
    #[serde(default)]
    pub base_commit: Option<String>,
    #[serde(default)]
    pub head_commit: Option<String>,
    pub status: String,
    #[serde(default)]
    pub status_reason: Option<String>,
    #[serde(default)]
    pub merge_commit: Option<String>,
    #[serde(default)]
    pub merge_memory_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectWorktree {
    pub worktree_id: String,
    pub project_id: String,
    #[serde(default)]
    pub branch_id: Option<String>,
    pub path: String,
    #[serde(default)]
    pub commit: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectEntity {
    pub entity_id: String,
    pub project_id: String,
    pub kind: String,
    pub key: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub source_memory_id: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectEntityInput {
    pub kind: String,
    pub key: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub source_memory_id: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectFreshnessEvent {
    pub event_id: String,
    pub project_id: String,
    pub memory_id: String,
    #[serde(default)]
    pub previous_commit: Option<String>,
    pub commit: String,
    pub action: String,
    #[serde(default)]
    pub reason: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMaintenanceReport {
    pub project_id: String,
    #[serde(default)]
    pub current_commit: Option<String>,
    pub stale_memory_ids: Vec<String>,
    pub superseded_memory_ids: Vec<String>,
    pub active_memory_ids: Vec<String>,
    pub checked_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMergeResult {
    pub branch: ProjectBranch,
    pub memory: Memory,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMembership {
    pub project_id: String,
    pub person_id: String,
    pub email: String,
    pub created_at: String,
}

pub const SHARE_REASON_MANUAL: &str = "manual";
pub const SHARE_REASON_AUTO: &str = "auto_nonsensitive_project_memory";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectSharePolicy {
    #[default]
    Manual,
    AutoNonsensitiveProjectMemory,
}

impl ProjectSharePolicy {
    pub fn parse(value: &str) -> Result<Self, StoreError> {
        match value.trim() {
            "manual" => Ok(Self::Manual),
            SHARE_REASON_AUTO => Ok(Self::AutoNonsensitiveProjectMemory),
            _ => Err(StoreError::Validation(
                "project share policy must be manual or auto_nonsensitive_project_memory"
                    .to_owned(),
            )),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectSharePolicyRecord {
    pub project_id: String,
    pub policy: ProjectSharePolicy,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectShareCandidate {
    pub memory_id: String,
    pub reason: String,
    pub not_auto_shared_reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShareCandidateDismissal {
    pub project_id: String,
    pub memory_id: String,
    pub dismissed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectConflict {
    pub memory_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SharedProjectMemory {
    #[serde(flatten)]
    pub memory: Memory,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_memory_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMemoryShare {
    pub project_id: String,
    pub memory_id: String,
    pub member_person_id: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectTeamMember {
    pub person_id: String,
    pub email: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectTeam {
    pub project_id: String,
    pub name: String,
    pub created_at: String,
    pub members: Vec<ProjectTeamMember>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProjectOwnerRecord {
    project_id: String,
    person_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MemoryOwnerRecord {
    memory_id: String,
    person_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProjectMemberRecord {
    project_id: String,
    owner_person_id: String,
    member_person_id: String,
    created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProjectShareRecord {
    project_id: String,
    memory_id: String,
    owner_person_id: String,
    member_person_id: String,
    created_at: String,
    #[serde(default)]
    revoked_at: Option<String>,
    #[serde(default)]
    share_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProjectPolicyRecord {
    project_id: String,
    policy: ProjectSharePolicy,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DismissedShareCandidate {
    project_id: String,
    memory_id: String,
    member_person_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProjectTeamRecord {
    project_id: String,
    name: String,
    created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ProjectTeamMemberRecord {
    project_id: String,
    member_person_id: String,
    created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredAuditEvent {
    person_id: String,
    audit_id: String,
    action: String,
    subject_id: Option<String>,
    actor_device_id: Option<String>,
    metadata: serde_json::Value,
    created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct DeviceRecord {
    device: Device,
    token: String,
}

pub struct MemoryStore {
    path: PathBuf,
    state: StoreState,
}

fn membership_view(record: &ProjectMemberRecord, email: &str) -> ProjectMembership {
    ProjectMembership {
        project_id: record.project_id.clone(),
        person_id: record.member_person_id.clone(),
        email: email.to_owned(),
        created_at: record.created_at.clone(),
    }
}

fn share_view(record: &ProjectShareRecord) -> ProjectMemoryShare {
    ProjectMemoryShare {
        project_id: record.project_id.clone(),
        memory_id: record.memory_id.clone(),
        member_person_id: record.member_person_id.clone(),
        created_at: record.created_at.clone(),
        revoked_at: record.revoked_at.clone(),
    }
}

fn team_member_view(record: &ProjectTeamMemberRecord, email: &str) -> ProjectTeamMember {
    ProjectTeamMember {
        person_id: record.member_person_id.clone(),
        email: email.to_owned(),
        created_at: record.created_at.clone(),
    }
}

fn share_audit_metadata(
    project_id: &str,
    member_person_id: &str,
    memory_id: &str,
) -> serde_json::Value {
    serde_json::json!({
        "project_id": project_id,
        "member_person_id": member_person_id,
        "memory_id": memory_id,
    })
}

fn owner_matches(owners: &[ProjectOwnerRecord], project_id: &str, person_id: &str) -> bool {
    owners
        .iter()
        .find(|record| record.project_id == project_id)
        .is_none_or(|record| record.person_id == person_id)
}

impl MemoryStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        let state = if path.exists() {
            let raw = fs::read_to_string(&path)?;
            let state: StoreState = serde_json::from_str(&raw)?;
            if state.version != STORE_VERSION {
                return Err(StoreError::Validation(format!(
                    "unsupported store version {}; expected {STORE_VERSION}",
                    state.version
                )));
            }
            state
        } else {
            StoreState::default()
        };

        let mut store = Self { path, state };
        store.sweep_expired()?;
        Ok(store)
    }

    pub fn sweep_expired(&mut self) -> Result<usize, StoreError> {
        let mut expired_ids = Vec::new();
        for observation in &self.state.observations {
            if is_expired(&observation.lifecycle) {
                expired_ids.push(observation.event_id.clone());
            }
        }
        for memory in &self.state.memories {
            if is_expired(&memory.lifecycle) {
                expired_ids.push(memory.memory_id.clone());
            }
        }

        let mut added = 0;
        for id in expired_ids {
            if self.state.forgotten_ids.insert(id.clone()) {
                added += 1;
            }
            if let Some(memory) = self
                .state
                .memories
                .iter()
                .find(|memory| memory.memory_id == id)
            {
                for event_id in &memory.source_event_ids {
                    if self.state.forgotten_ids.insert(event_id.clone()) {
                        added += 1;
                    }
                }
            }
        }
        if added > 0 {
            self.persist()?;
        }
        Ok(added)
    }

    pub fn start_session(
        &mut self,
        session_id: impl Into<String>,
        context: WorkContext,
    ) -> Result<Session, StoreError> {
        let session_id = session_id.into();
        if session_id.trim().is_empty() {
            return Err(StoreError::Validation(
                "session_id must not be empty".to_owned(),
            ));
        }

        if let Some(session) = self
            .state
            .sessions
            .iter()
            .find(|session| session.session_id == session_id)
        {
            return Ok(session.clone());
        }

        let session = Session {
            session_id,
            context,
            started_at: timestamp(),
            ended_at: None,
        };
        self.state.sessions.push(session.clone());
        self.persist()?;
        Ok(session)
    }

    pub fn bootstrap_account(
        &mut self,
        email: &str,
        password: &str,
    ) -> Result<AuthSessionResult, StoreError> {
        if !self.state.accounts.is_empty() {
            return Err(StoreError::Validation(
                "account bootstrap has already completed".to_owned(),
            ));
        }
        let email = normalize_email(email)?;
        validate_password(password)?;
        let account = Account {
            person_id: format!("person-{}", timestamp_nanos()),
            email,
            status: AccountStatus::Active,
            created_at: timestamp(),
        };
        self.state.accounts.push(AccountRecord {
            account: account.clone(),
            password_hash: hash_password(password)?,
        });
        let session = self.issue_auth_session(account)?;
        self.persist()?;
        Ok(session)
    }

    pub fn register_account(
        &mut self,
        email: &str,
        password: &str,
    ) -> Result<AuthSessionResult, StoreError> {
        if self.state.accounts.is_empty() {
            return Err(StoreError::Validation(
                "account bootstrap has not completed".to_owned(),
            ));
        }
        let email = normalize_email(email)?;
        validate_password(password)?;
        if self
            .state
            .accounts
            .iter()
            .any(|record| record.account.email == email)
        {
            return Err(StoreError::Validation(
                "account email is already registered".to_owned(),
            ));
        }
        let account = Account {
            person_id: format!("person-{}", timestamp_nanos()),
            email,
            status: AccountStatus::Active,
            created_at: timestamp(),
        };
        self.state.accounts.push(AccountRecord {
            account: account.clone(),
            password_hash: hash_password(password)?,
        });
        let session = self.issue_auth_session(account)?;
        self.persist()?;
        Ok(session)
    }

    pub fn login(&mut self, email: &str, password: &str) -> Result<AuthSessionResult, StoreError> {
        let email = normalize_email(email)?;
        let record = self
            .state
            .accounts
            .iter()
            .find(|record| record.account.email == email)
            .cloned()
            .ok_or_else(|| StoreError::Validation("invalid account credentials".to_owned()))?;
        if record.account.status != AccountStatus::Active
            || !verify_password(password, &record.password_hash)
        {
            return Err(StoreError::Validation(
                "invalid account credentials".to_owned(),
            ));
        }
        let session = self.issue_auth_session(record.account)?;
        self.persist()?;
        Ok(session)
    }

    pub fn authenticate_account(&self, token: &str) -> Result<Account, StoreError> {
        let token_hash = hash_token(token);
        let session = self
            .state
            .auth_sessions
            .iter()
            .find(|session| session.token_hash == token_hash)
            .ok_or_else(|| StoreError::Validation("account authentication failed".to_owned()))?;
        if session.revoked_at.is_some()
            || session
                .expires_at
                .parse::<u64>()
                .ok()
                .is_some_and(|expires_at| expires_at <= now_seconds())
        {
            return Err(StoreError::Validation(
                "account session is expired or revoked".to_owned(),
            ));
        }
        let account = self
            .state
            .accounts
            .iter()
            .find(|record| record.account.person_id == session.person_id)
            .map(|record| record.account.clone())
            .ok_or_else(|| StoreError::Validation("account authentication failed".to_owned()))?;
        if account.status != AccountStatus::Active {
            return Err(StoreError::Validation("account is disabled".to_owned()));
        }
        Ok(account)
    }

    pub fn logout_account(&mut self, token: &str) -> Result<(), StoreError> {
        let token_hash = hash_token(token);
        let session = self
            .state
            .auth_sessions
            .iter_mut()
            .find(|session| session.token_hash == token_hash)
            .ok_or_else(|| StoreError::Validation("account authentication failed".to_owned()))?;
        if session.revoked_at.is_none() {
            session.revoked_at = Some(timestamp());
            self.persist()?;
        }
        Ok(())
    }

    pub fn register_project(
        &mut self,
        account_token: &str,
        repository: &str,
        name: &str,
        default_branch: Option<String>,
    ) -> Result<Project, StoreError> {
        let account = self.authenticate_account(account_token)?;
        let repository = repository.trim();
        let name = name.trim();
        if repository.is_empty() || name.is_empty() {
            return Err(StoreError::Validation(
                "repository and project name must not be empty".to_owned(),
            ));
        }
        let existing = {
            let owners = &self.state.project_owners;
            self.state
                .projects
                .iter()
                .find(|project| {
                    project.repository == repository
                        && owner_matches(owners, &project.project_id, &account.person_id)
                })
                .cloned()
        };
        if let Some(project) = existing {
            if self.project_owner_id(&project.project_id).is_none() {
                self.claim_project_owner(&account.person_id, &project.project_id);
                self.persist()?;
            }
            return Ok(project);
        }
        let now = timestamp();
        let project = Project {
            project_id: format!("project-{}", &hash_token(&random_token("project"))[..24]),
            repository: repository.to_owned(),
            name: name.to_owned(),
            default_branch,
            created_at: now.clone(),
            updated_at: now,
        };
        self.state.projects.push(project.clone());
        self.claim_project_owner(&account.person_id, &project.project_id);
        self.persist()?;
        Ok(project)
    }

    pub fn projects(&self, account_token: &str) -> Result<Vec<Project>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        let owners = &self.state.project_owners;
        Ok(self
            .state
            .projects
            .iter()
            .filter(|project| {
                owner_matches(owners, &project.project_id, &account.person_id)
                    || self.state.project_members.iter().any(|member| {
                        member.project_id == project.project_id
                            && member.member_person_id == account.person_id
                    })
            })
            .cloned()
            .collect())
    }

    pub fn register_branch(
        &mut self,
        account_token: &str,
        project_id: &str,
        name: &str,
        base_branch: Option<String>,
        base_commit: Option<String>,
        head_commit: Option<String>,
    ) -> Result<ProjectBranch, StoreError> {
        let _account = self.authenticate_account(account_token)?;
        validate_project_text(project_id, "project_id")?;
        validate_project_text(name, "branch name")?;
        if !self
            .state
            .projects
            .iter()
            .any(|project| project.project_id == project_id)
        {
            return Err(StoreError::Validation(format!(
                "unknown project: {project_id}"
            )));
        }
        if let Some(branch) = self
            .state
            .project_branches
            .iter()
            .find(|branch| branch.project_id == project_id && branch.name == name.trim())
        {
            return Ok(branch.clone());
        }
        let now = timestamp();
        let branch = ProjectBranch {
            branch_id: format!("branch-{}", &hash_token(&random_token("branch"))[..24]),
            project_id: project_id.to_owned(),
            name: name.trim().to_owned(),
            base_branch,
            base_commit,
            head_commit,
            status: "active".to_owned(),
            status_reason: None,
            merge_commit: None,
            merge_memory_id: None,
            created_at: now.clone(),
            updated_at: now,
        };
        self.state.project_branches.push(branch.clone());
        self.persist()?;
        Ok(branch)
    }

    pub fn branches(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectBranch>, StoreError> {
        let _account = self.authenticate_account(account_token)?;
        validate_project_text(project_id, "project_id")?;
        if !self
            .state
            .projects
            .iter()
            .any(|project| project.project_id == project_id)
        {
            return Err(StoreError::Validation(format!(
                "unknown project: {project_id}"
            )));
        }
        Ok(self
            .state
            .project_branches
            .iter()
            .filter(|branch| branch.project_id == project_id)
            .cloned()
            .collect())
    }

    pub fn register_worktree(
        &mut self,
        account_token: &str,
        project_id: &str,
        branch_id: Option<String>,
        path: &str,
        commit: Option<String>,
    ) -> Result<ProjectWorktree, StoreError> {
        let _account = self.authenticate_account(account_token)?;
        validate_project_text(project_id, "project_id")?;
        validate_project_text(path, "worktree path")?;
        if !self
            .state
            .projects
            .iter()
            .any(|project| project.project_id == project_id)
        {
            return Err(StoreError::Validation(format!(
                "unknown project: {project_id}"
            )));
        }
        if let Some(branch_id) = &branch_id
            && !self
                .state
                .project_branches
                .iter()
                .any(|branch| branch.project_id == project_id && branch.branch_id == *branch_id)
        {
            return Err(StoreError::Validation(format!(
                "unknown project branch: {branch_id}"
            )));
        }
        if let Some(worktree) = self
            .state
            .project_worktrees
            .iter()
            .find(|worktree| worktree.project_id == project_id && worktree.path == path.trim())
        {
            return Ok(worktree.clone());
        }
        let now = timestamp();
        let worktree = ProjectWorktree {
            worktree_id: format!("worktree-{}", &hash_token(&random_token("worktree"))[..24]),
            project_id: project_id.to_owned(),
            branch_id,
            path: path.trim().to_owned(),
            commit,
            status: "active".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        };
        self.state.project_worktrees.push(worktree.clone());
        self.persist()?;
        Ok(worktree)
    }

    pub fn worktrees(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectWorktree>, StoreError> {
        let _account = self.authenticate_account(account_token)?;
        validate_project_text(project_id, "project_id")?;
        if !self
            .state
            .projects
            .iter()
            .any(|project| project.project_id == project_id)
        {
            return Err(StoreError::Validation(format!(
                "unknown project: {project_id}"
            )));
        }
        Ok(self
            .state
            .project_worktrees
            .iter()
            .filter(|worktree| worktree.project_id == project_id)
            .cloned()
            .collect())
    }

    pub fn register_project_entity(
        &mut self,
        account_token: &str,
        project_id: &str,
        input: ProjectEntityInput,
    ) -> Result<ProjectEntity, StoreError> {
        let _account = self.authenticate_account(account_token)?;
        validate_project_text(project_id, "project_id")?;
        validate_project_text(&input.key, "entity key")?;
        validate_entity_kind(&input.kind)?;
        let project = self
            .state
            .projects
            .iter()
            .find(|project| project.project_id == project_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown project: {project_id}")))?;
        if let Some(memory_id) = &input.source_memory_id {
            let memory = self
                .state
                .memories
                .iter()
                .find(|memory| memory.memory_id == *memory_id)
                .ok_or_else(|| StoreError::Validation(format!("unknown memory: {memory_id}")))?;
            if memory.context.repository.as_deref() != Some(project.repository.as_str()) {
                return Err(StoreError::Validation(
                    "entity source repository does not match the project".to_owned(),
                ));
            }
        }
        let key = input.key.trim();
        if let Some(entity) = self.state.project_entities.iter().find(|entity| {
            entity.project_id == project_id && entity.kind == input.kind && entity.key == key
        }) {
            return Ok(entity.clone());
        }
        let content = input
            .content
            .map(|value| redact_sensitive_content(value.trim()))
            .filter(|value| !value.is_empty());
        if content
            .as_ref()
            .is_some_and(|value| value.len() > MAX_HANDOFF_ITEM_BYTES)
        {
            return Err(StoreError::Validation(
                "entity content exceeds the 2048 byte limit".to_owned(),
            ));
        }
        let commit = input
            .commit
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if commit.as_ref().is_some_and(|value| value.len() > 256) {
            return Err(StoreError::Validation(
                "entity commit exceeds the 256 byte limit".to_owned(),
            ));
        }
        let now = timestamp();
        let entity = ProjectEntity {
            entity_id: format!("entity-{}", &hash_token(&random_token("entity"))[..24]),
            project_id: project_id.to_owned(),
            kind: input.kind,
            key: key.to_owned(),
            content,
            source_memory_id: input.source_memory_id,
            commit,
            status: "active".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        };
        self.state.project_entities.push(entity.clone());
        self.persist()?;
        Ok(entity)
    }

    pub fn project_entities(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectEntity>, StoreError> {
        let _account = self.authenticate_account(account_token)?;
        validate_project_text(project_id, "project_id")?;
        if !self
            .state
            .projects
            .iter()
            .any(|project| project.project_id == project_id)
        {
            return Err(StoreError::Validation(format!(
                "unknown project: {project_id}"
            )));
        }
        Ok(self
            .state
            .project_entities
            .iter()
            .filter(|entity| entity.project_id == project_id && entity.status == "active")
            .cloned()
            .collect())
    }

    pub fn freshness_history(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectFreshnessEvent>, StoreError> {
        let _account = self.authenticate_account(account_token)?;
        validate_project_text(project_id, "project_id")?;
        if !self
            .state
            .projects
            .iter()
            .any(|project| project.project_id == project_id)
        {
            return Err(StoreError::Validation(format!(
                "unknown project: {project_id}"
            )));
        }
        Ok(self
            .state
            .freshness_events
            .iter()
            .filter(|event| event.project_id == project_id)
            .cloned()
            .collect())
    }

    pub fn project_maintenance(
        &self,
        account_token: &str,
        project_id: &str,
        current_commit: Option<&str>,
    ) -> Result<ProjectMaintenanceReport, StoreError> {
        let _account = self.authenticate_account(account_token)?;
        validate_project_text(project_id, "project_id")?;
        if !self
            .state
            .projects
            .iter()
            .any(|project| project.project_id == project_id)
        {
            return Err(StoreError::Validation(format!(
                "unknown project: {project_id}"
            )));
        }
        let project_memories = self
            .state
            .memories
            .iter()
            .filter(|memory| {
                memory.project_id.as_deref() == Some(project_id)
                    && !self.memory_is_forgotten(memory)
                    && !is_expired(&memory.lifecycle)
            })
            .collect::<Vec<_>>();
        let superseded_memory_ids = project_memories
            .iter()
            .filter(|memory| {
                project_memories.iter().any(|candidate| {
                    candidate.supersedes.as_deref() == Some(memory.memory_id.as_str())
                })
            })
            .map(|memory| memory.memory_id.clone())
            .collect::<Vec<_>>();
        let stale_memory_ids = current_commit
            .map(|commit| {
                project_memories
                    .iter()
                    .filter(|memory| {
                        memory
                            .verified_commit
                            .as_deref()
                            .is_some_and(|verified| verified != commit)
                    })
                    .map(|memory| memory.memory_id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let active_memory_ids = project_memories
            .iter()
            .filter(|memory| {
                !superseded_memory_ids
                    .iter()
                    .any(|id| id == &memory.memory_id)
            })
            .map(|memory| memory.memory_id.clone())
            .collect();
        Ok(ProjectMaintenanceReport {
            project_id: project_id.to_owned(),
            current_commit: current_commit.map(str::to_owned),
            stale_memory_ids,
            superseded_memory_ids,
            active_memory_ids,
            checked_at: timestamp(),
        })
    }

    fn index_project_memory(&mut self, project_id: &str, memory: &Memory) {
        let mut candidates = vec![("decision", memory.memory_id.clone(), None)];
        candidates.extend(
            memory
                .context
                .files
                .iter()
                .cloned()
                .map(|value| ("file", value, None)),
        );
        candidates.extend(
            memory
                .context
                .symbols
                .iter()
                .cloned()
                .map(|value| ("symbol", value, None)),
        );
        candidates.extend(
            memory
                .context
                .handoffs
                .iter()
                .cloned()
                .map(|value| ("handoff", value, Some(memory.content.clone()))),
        );
        candidates.extend(
            memory
                .context
                .runbooks
                .iter()
                .cloned()
                .map(|value| ("runbook", value, Some(memory.content.clone()))),
        );
        for (kind, key, content) in candidates {
            if self.state.project_entities.iter().any(|entity| {
                entity.project_id == project_id && entity.kind == kind && entity.key == key
            }) {
                continue;
            }
            let now = timestamp();
            self.state.project_entities.push(ProjectEntity {
                entity_id: format!("entity-{}", &hash_token(&random_token("entity"))[..24]),
                project_id: project_id.to_owned(),
                kind: kind.to_owned(),
                key,
                content,
                source_memory_id: Some(memory.memory_id.clone()),
                commit: memory.verified_commit.clone(),
                status: "active".to_owned(),
                created_at: now.clone(),
                updated_at: now,
            });
        }
    }

    fn record_freshness_event(
        &mut self,
        project_id: &str,
        memory_id: &str,
        previous_commit: Option<String>,
        commit: &str,
        action: &str,
        reason: Option<String>,
    ) {
        self.state.freshness_events.push(ProjectFreshnessEvent {
            event_id: format!(
                "freshness-{}",
                &hash_token(&random_token("freshness"))[..24]
            ),
            project_id: project_id.to_owned(),
            memory_id: memory_id.to_owned(),
            previous_commit,
            commit: commit.to_owned(),
            action: action.to_owned(),
            reason,
            created_at: timestamp(),
        });
    }

    pub fn confirm_branch_merge(
        &mut self,
        account_token: &str,
        project_id: &str,
        branch_id: &str,
        memory_id: &str,
        commit: &str,
        reason: &str,
    ) -> Result<ProjectMergeResult, StoreError> {
        let account = self.authenticate_account(account_token)?;
        validate_project_text(project_id, "project_id")?;
        validate_project_text(branch_id, "branch_id")?;
        validate_project_text(memory_id, "memory_id")?;
        let commit = commit.trim();
        let reason = reason.trim();
        if commit.is_empty() || reason.is_empty() {
            return Err(StoreError::Validation(
                "merge commit and confirmation reason are required".to_owned(),
            ));
        }
        if commit.len() > 256 {
            return Err(StoreError::Validation(
                "commit exceeds the 256 byte limit".to_owned(),
            ));
        }
        if reason.len() > MAX_HANDOFF_ITEM_BYTES {
            return Err(StoreError::Validation(
                "merge reason exceeds the 2048 byte limit".to_owned(),
            ));
        }
        let project = self
            .state
            .projects
            .iter()
            .find(|project| project.project_id == project_id)
            .cloned()
            .ok_or_else(|| StoreError::Validation(format!("unknown project: {project_id}")))?;
        let branch = self
            .state
            .project_branches
            .iter()
            .find(|branch| branch.project_id == project_id && branch.branch_id == branch_id)
            .cloned()
            .ok_or_else(|| {
                StoreError::Validation(format!("unknown project branch: {branch_id}"))
            })?;
        if branch.status == "merged" {
            if branch.merge_commit.as_deref() == Some(commit)
                && let Some(merged_memory_id) = &branch.merge_memory_id
                && let Some(memory) = self.state.memories.iter().find(|memory| {
                    memory.memory_id == *merged_memory_id
                        && memory.promoted_from.as_deref() == Some(memory_id)
                })
            {
                return Ok(ProjectMergeResult {
                    branch,
                    memory: memory.clone(),
                });
            }
            return Err(StoreError::Validation(
                "project branch is already merged".to_owned(),
            ));
        }
        if branch.status == "closed" {
            return Err(StoreError::Validation(
                "closed project branches cannot be merged".to_owned(),
            ));
        }
        let source = self
            .state
            .memories
            .iter()
            .find(|memory| memory.memory_id == memory_id)
            .cloned()
            .ok_or_else(|| StoreError::Validation(format!("unknown memory: {memory_id}")))?;
        if source.context.repository.as_deref() != Some(project.repository.as_str()) {
            return Err(StoreError::Validation(
                "memory repository does not match the project".to_owned(),
            ));
        }
        if source.context.branch.as_deref() != Some(branch.name.as_str()) {
            return Err(StoreError::Validation(
                "memory branch does not match the registered project branch".to_owned(),
            ));
        }
        if self.memory_is_forgotten(&source) || is_expired(&source.lifecycle) {
            return Err(StoreError::Validation(
                "forgotten or expired memories cannot be promoted".to_owned(),
            ));
        }
        if source.lifecycle.sensitivity == Sensitivity::Secret {
            return Err(StoreError::Validation(
                "secret memories cannot be promoted".to_owned(),
            ));
        }

        let mut created_project_memory = false;
        let promoted = if let Some(existing) = self.state.memories.iter().find(|memory| {
            memory.project_id.as_deref() == Some(project_id)
                && memory.promoted_from.as_deref() == Some(memory_id)
        }) {
            existing.clone()
        } else {
            let mut promoted = source;
            promoted.memory_id = format!(
                "memory-project-{}",
                &hash_token(&random_token("promotion"))[..24]
            );
            promoted.kind = MemoryKind::Decision;
            promoted.scope = MemoryScope::Project;
            promoted.project_id = Some(project_id.to_owned());
            promoted.context.repository = Some(project.repository);
            promoted.context.branch = None;
            promoted.context.worktree = None;
            promoted.context.commit = None;
            promoted.promoted_from = Some(memory_id.to_owned());
            promoted.promotion_reason = Some(redact_sensitive_content(reason));
            promoted.lifecycle.retention = Retention::Durable;
            promoted.lifecycle.expires_at = None;
            promoted.created_at = timestamp();
            self.state.memories.push(promoted.clone());
            created_project_memory = true;
            promoted
        };
        let promoted_index = self
            .state
            .memories
            .iter()
            .position(|memory| memory.memory_id == promoted.memory_id)
            .ok_or_else(|| {
                StoreError::Validation("promoted project decision is unavailable".to_owned())
            })?;
        let promoted = &mut self.state.memories[promoted_index];
        let previous_commit = promoted.verified_commit.clone();
        promoted.verified_commit = Some(commit.to_owned());
        promoted.lifecycle.freshness = 100;
        let memory = promoted.clone();
        self.index_project_memory(project_id, &memory);
        self.record_freshness_event(
            project_id,
            &memory.memory_id,
            previous_commit,
            commit,
            "merge",
            Some(redact_sensitive_content(reason)),
        );
        let branch_index = self
            .state
            .project_branches
            .iter()
            .position(|branch| branch.project_id == project_id && branch.branch_id == branch_id)
            .ok_or_else(|| {
                StoreError::Validation(format!("unknown project branch: {branch_id}"))
            })?;
        let branch = &mut self.state.project_branches[branch_index];
        branch.status = "merged".to_owned();
        branch.status_reason = Some(redact_sensitive_content(reason));
        branch.merge_commit = Some(commit.to_owned());
        branch.merge_memory_id = Some(memory.memory_id.clone());
        branch.updated_at = timestamp();
        let branch = branch.clone();
        for worktree in &mut self.state.project_worktrees {
            if worktree.project_id == project_id && worktree.branch_id.as_deref() == Some(branch_id)
            {
                worktree.status = "removed".to_owned();
                worktree.updated_at = timestamp();
            }
        }
        self.record_memory_owner(&account.person_id, &memory.memory_id);
        if created_project_memory {
            self.auto_share_new_project_memory(&account.person_id, project_id, &memory);
        }
        self.persist()?;
        Ok(ProjectMergeResult { branch, memory })
    }

    pub fn close_branch(
        &mut self,
        account_token: &str,
        project_id: &str,
        branch_id: &str,
        reason: &str,
    ) -> Result<ProjectBranch, StoreError> {
        let _account = self.authenticate_account(account_token)?;
        validate_project_text(project_id, "project_id")?;
        validate_project_text(branch_id, "branch_id")?;
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(StoreError::Validation(
                "branch close reason is required".to_owned(),
            ));
        }
        if reason.len() > MAX_HANDOFF_ITEM_BYTES {
            return Err(StoreError::Validation(
                "branch close reason exceeds the 2048 byte limit".to_owned(),
            ));
        }
        let branch = self
            .state
            .project_branches
            .iter_mut()
            .find(|branch| branch.project_id == project_id && branch.branch_id == branch_id)
            .ok_or_else(|| {
                StoreError::Validation(format!("unknown project branch: {branch_id}"))
            })?;
        if branch.status == "merged" {
            return Err(StoreError::Validation(
                "merged project branches cannot be closed".to_owned(),
            ));
        }
        if branch.status == "closed" {
            return Ok(branch.clone());
        }
        branch.status = "closed".to_owned();
        branch.status_reason = Some(redact_sensitive_content(reason));
        branch.updated_at = timestamp();
        let result = branch.clone();
        for worktree in &mut self.state.project_worktrees {
            if worktree.project_id == project_id && worktree.branch_id.as_deref() == Some(branch_id)
            {
                worktree.status = "removed".to_owned();
                worktree.updated_at = timestamp();
            }
        }
        self.persist()?;
        Ok(result)
    }

    pub fn promote_memory(
        &mut self,
        account_token: &str,
        project_id: &str,
        memory_id: &str,
        reason: &str,
    ) -> Result<Memory, StoreError> {
        let account = self.authenticate_account(account_token)?;
        let reason = reason.trim();
        if project_id.trim().is_empty() || memory_id.trim().is_empty() || reason.is_empty() {
            return Err(StoreError::Validation(
                "project_id, memory_id, and promotion reason are required".to_owned(),
            ));
        }
        if reason.len() > MAX_HANDOFF_ITEM_BYTES {
            return Err(StoreError::Validation(
                "promotion reason exceeds the 2048 byte limit".to_owned(),
            ));
        }
        let project = self
            .state
            .projects
            .iter()
            .find(|project| project.project_id == project_id)
            .cloned()
            .ok_or_else(|| StoreError::Validation(format!("unknown project: {project_id}")))?;
        let source = self
            .state
            .memories
            .iter()
            .find(|memory| memory.memory_id == memory_id)
            .cloned()
            .ok_or_else(|| StoreError::Validation(format!("unknown memory: {memory_id}")))?;
        if self.memory_is_forgotten(&source) || is_expired(&source.lifecycle) {
            return Err(StoreError::Validation(
                "forgotten or expired memories cannot be promoted".to_owned(),
            ));
        }
        if source.lifecycle.sensitivity == Sensitivity::Secret {
            return Err(StoreError::Validation(
                "secret memories cannot be promoted".to_owned(),
            ));
        }
        if source.context.repository.as_deref() != Some(project.repository.as_str()) {
            return Err(StoreError::Validation(
                "memory repository does not match the project".to_owned(),
            ));
        }
        if let Some(existing) = self.state.memories.iter().find(|memory| {
            memory.project_id.as_deref() == Some(project_id)
                && memory.promoted_from.as_deref() == Some(memory_id)
        }) {
            let existing = existing.clone();
            self.record_memory_owner(&account.person_id, &existing.memory_id);
            self.persist()?;
            return Ok(existing);
        }
        let mut promoted = source;
        promoted.memory_id = format!(
            "memory-project-{}",
            &hash_token(&random_token("promotion"))[..24]
        );
        promoted.kind = MemoryKind::Decision;
        promoted.scope = MemoryScope::Project;
        promoted.project_id = Some(project_id.to_owned());
        promoted.context.repository = Some(project.repository.clone());
        promoted.context.branch = None;
        promoted.context.worktree = None;
        promoted.context.commit = None;
        promoted.promoted_from = Some(memory_id.to_owned());
        promoted.promotion_reason = Some(redact_sensitive_content(reason));
        promoted.verified_commit = self
            .state
            .memories
            .iter()
            .find(|memory| memory.memory_id == memory_id)
            .and_then(|memory| memory.context.commit.clone());
        promoted.lifecycle.retention = Retention::Durable;
        promoted.lifecycle.expires_at = None;
        promoted.created_at = timestamp();
        self.state.memories.push(promoted.clone());
        self.record_memory_owner(&account.person_id, &promoted.memory_id);
        self.index_project_memory(project_id, &promoted);
        if let Some(commit) = promoted.verified_commit.clone() {
            self.record_freshness_event(
                project_id,
                &promoted.memory_id,
                None,
                &commit,
                "promotion",
                Some(redact_sensitive_content(reason)),
            );
        }
        self.auto_share_new_project_memory(&account.person_id, project_id, &promoted);
        self.record_share_audit(
            &account.person_id,
            "memory_promote",
            Some(memory_id),
            serde_json::json!({
                "project_id": project_id,
                "promotion_id": promoted.memory_id,
            }),
        );
        self.persist()?;
        Ok(promoted)
    }

    pub fn confirm_memory(
        &mut self,
        account_token: &str,
        project_id: &str,
        memory_id: &str,
        commit: Option<&str>,
        reason: &str,
    ) -> Result<Memory, StoreError> {
        let promoted = self.promote_memory(account_token, project_id, memory_id, reason)?;
        match commit.map(str::trim).filter(|value| !value.is_empty()) {
            Some(commit) => {
                self.verify_project_memory(account_token, project_id, &promoted.memory_id, commit)
            }
            None => Ok(promoted),
        }
    }

    pub fn verify_project_memory(
        &mut self,
        account_token: &str,
        project_id: &str,
        memory_id: &str,
        commit: &str,
    ) -> Result<Memory, StoreError> {
        let _account = self.authenticate_account(account_token)?;
        let commit = commit.trim();
        if project_id.trim().is_empty() || memory_id.trim().is_empty() || commit.is_empty() {
            return Err(StoreError::Validation(
                "project_id, memory_id, and commit are required".to_owned(),
            ));
        }
        if commit.len() > 256 {
            return Err(StoreError::Validation(
                "commit exceeds the 256 byte limit".to_owned(),
            ));
        }
        if !self
            .state
            .projects
            .iter()
            .any(|project| project.project_id == project_id)
        {
            return Err(StoreError::Validation(format!(
                "unknown project: {project_id}"
            )));
        }
        let memory = self
            .state
            .memories
            .iter_mut()
            .find(|memory| {
                memory.memory_id == memory_id
                    && memory.project_id.as_deref() == Some(project_id)
                    && memory.scope == MemoryScope::Project
            })
            .ok_or_else(|| {
                StoreError::Validation(format!("unknown project memory: {memory_id}"))
            })?;
        let previous_commit = memory.verified_commit.clone();
        memory.verified_commit = Some(commit.to_owned());
        memory.lifecycle.freshness = 100;
        let result = memory.clone();
        self.record_freshness_event(
            project_id,
            memory_id,
            previous_commit,
            commit,
            "verification",
            None,
        );
        self.persist()?;
        Ok(result)
    }

    pub fn project_memories(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<Memory>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        Ok(self.live_project_memories(project_id))
    }

    pub fn audit_events(
        &self,
        account_token: &str,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        Ok(self
            .state
            .audit_events
            .iter()
            .rev()
            .filter(|event| event.person_id == account.person_id)
            .take(limit)
            .map(|event| AuditEvent {
                audit_id: event.audit_id.clone(),
                action: event.action.clone(),
                subject_id: event.subject_id.clone(),
                actor_device_id: event.actor_device_id.clone(),
                metadata: event.metadata.clone(),
                created_at: event.created_at.clone(),
            })
            .collect())
    }

    pub fn invite_project_member(
        &mut self,
        account_token: &str,
        project_id: &str,
        email: &str,
    ) -> Result<ProjectMembership, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        let email = normalize_email(email)?;
        if email == account.email {
            return Err(StoreError::Validation(
                "project owner cannot invite themselves".to_owned(),
            ));
        }
        let member = self.account_by_email(&email)?;
        if let Some(existing) = self.state.project_members.iter().find(|record| {
            record.project_id == project_id && record.member_person_id == member.person_id
        }) {
            return Ok(membership_view(existing, &member.email));
        }
        let record = ProjectMemberRecord {
            project_id: project_id.to_owned(),
            owner_person_id: account.person_id.clone(),
            member_person_id: member.person_id.clone(),
            created_at: timestamp(),
        };
        self.record_share_audit(
            &account.person_id,
            "project_member_invited",
            Some(&member.person_id),
            serde_json::json!({
                "project_id": project_id,
                "member_person_id": member.person_id,
            }),
        );
        self.state.project_members.push(record.clone());
        self.persist()?;
        Ok(membership_view(&record, &member.email))
    }

    pub fn share_project_memory(
        &mut self,
        account_token: &str,
        project_id: &str,
        memory_id: &str,
        email: &str,
    ) -> Result<ProjectMemoryShare, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        let member = self.account_by_email(&normalize_email(email)?)?;
        if member.person_id == account.person_id {
            return Err(StoreError::Validation(
                "project owner cannot invite themselves".to_owned(),
            ));
        }
        if !self.state.project_members.iter().any(|record| {
            record.project_id == project_id && record.member_person_id == member.person_id
        }) {
            return Err(StoreError::Validation(
                "account is not a project member".to_owned(),
            ));
        }
        let memory = self.shareable_project_memory(project_id, memory_id)?;
        let share = self.grant_project_share(
            &account.person_id,
            project_id,
            &memory.memory_id,
            &member.person_id,
            SHARE_REASON_MANUAL,
        );
        self.persist()?;
        Ok(share)
    }

    pub fn shared_project_memories(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<SharedProjectMemory>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        validate_project_text(project_id, "project_id")?;
        if !self
            .state
            .projects
            .iter()
            .any(|project| project.project_id == project_id)
        {
            return Err(StoreError::Validation(format!(
                "unknown project: {project_id}"
            )));
        }
        if !self.state.project_members.iter().any(|record| {
            record.project_id == project_id && record.member_person_id == account.person_id
        }) {
            let had_share = self.state.project_shares.iter().any(|share| {
                share.project_id == project_id && share.member_person_id == account.person_id
            });
            if had_share {
                return Ok(Vec::new());
            }
            return Err(StoreError::Validation(
                "account is not a project member".to_owned(),
            ));
        }
        let mut memories = self
            .state
            .memories
            .iter()
            .filter(|memory| {
                self.state.project_shares.iter().any(|share| {
                    share.project_id == project_id
                        && share.memory_id == memory.memory_id
                        && share.member_person_id == account.person_id
                        && share.revoked_at.is_none()
                }) && !self.memory_is_forgotten(memory)
                    && !is_expired(&memory.lifecycle)
                    && !self.memory_is_superseded(memory)
            })
            .map(|memory| SharedProjectMemory {
                share_reason: self
                    .state
                    .project_shares
                    .iter()
                    .find(|share| {
                        share.project_id == project_id
                            && share.memory_id == memory.memory_id
                            && share.member_person_id == account.person_id
                            && share.revoked_at.is_none()
                    })
                    .and_then(|share| share.share_reason.clone()),
                source_memory_id: memory.promoted_from.clone(),
                memory: memory.clone(),
            })
            .collect::<Vec<_>>();
        memories.sort_by(|left, right| right.memory.created_at.cmp(&left.memory.created_at));
        Ok(memories)
    }

    pub fn revoke_project_share(
        &mut self,
        account_token: &str,
        project_id: &str,
        memory_id: &str,
        email: &str,
    ) -> Result<ProjectMemoryShare, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        let member = self.account_by_email(&normalize_email(email)?)?;
        let revoked = self
            .state
            .project_shares
            .iter_mut()
            .find(|share| {
                share.project_id == project_id
                    && share.memory_id == memory_id
                    && share.member_person_id == member.person_id
                    && share.revoked_at.is_none()
            })
            .map(|share| {
                share.revoked_at = Some(timestamp());
                share_view(share)
            })
            .ok_or_else(|| StoreError::Validation("unknown project share".to_owned()))?;
        self.record_share_audit(
            &account.person_id,
            "project_memory_share_revoked",
            Some(memory_id),
            share_audit_metadata(project_id, &member.person_id, memory_id),
        );
        self.persist()?;
        Ok(revoked)
    }

    pub fn create_project_team(
        &mut self,
        account_token: &str,
        project_id: &str,
        name: &str,
    ) -> Result<ProjectTeam, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        let name = name.trim();
        validate_project_text(name, "name")?;
        if let Some(existing) = self
            .state
            .project_teams
            .iter()
            .find(|team| team.project_id == project_id)
            .cloned()
        {
            if existing.name == name {
                return self.team_view(&existing);
            }
            return Err(StoreError::Validation(
                "project already has a team space".to_owned(),
            ));
        }
        let record = ProjectTeamRecord {
            project_id: project_id.to_owned(),
            name: name.to_owned(),
            created_at: timestamp(),
        };
        self.record_share_audit(
            &account.person_id,
            "project_team_created",
            Some(project_id),
            serde_json::json!({ "project_id": project_id }),
        );
        self.state.project_teams.push(record.clone());
        self.persist()?;
        self.team_view(&record)
    }

    pub fn add_project_team_member(
        &mut self,
        account_token: &str,
        project_id: &str,
        email: &str,
    ) -> Result<ProjectTeamMember, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        self.require_project_team(project_id)?;
        let member = self.member_account(project_id, email)?;
        if let Some(existing) = self
            .state
            .project_team_members
            .iter()
            .find(|record| {
                record.project_id == project_id && record.member_person_id == member.person_id
            })
            .cloned()
        {
            return Ok(team_member_view(&existing, &member.email));
        }
        let record = ProjectTeamMemberRecord {
            project_id: project_id.to_owned(),
            member_person_id: member.person_id.clone(),
            created_at: timestamp(),
        };
        self.record_share_audit(
            &account.person_id,
            "project_team_member_added",
            Some(&member.person_id),
            serde_json::json!({
                "project_id": project_id,
                "member_person_id": member.person_id,
            }),
        );
        self.state.project_team_members.push(record.clone());
        self.persist()?;
        Ok(team_member_view(&record, &member.email))
    }

    pub fn remove_project_team_member(
        &mut self,
        account_token: &str,
        project_id: &str,
        email: &str,
    ) -> Result<ProjectTeamMember, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        let member = self.account_by_email(&normalize_email(email)?)?;
        let existing = self
            .state
            .project_team_members
            .iter()
            .find(|record| {
                record.project_id == project_id && record.member_person_id == member.person_id
            })
            .cloned()
            .ok_or_else(|| StoreError::Validation("account is not a team member".to_owned()))?;
        let removed_at = timestamp();
        for share in &mut self.state.project_shares {
            if share.project_id == project_id
                && share.member_person_id == member.person_id
                && share.revoked_at.is_none()
            {
                share.revoked_at = Some(removed_at.clone());
            }
        }
        self.state.project_members.retain(|record| {
            record.project_id != project_id || record.member_person_id != member.person_id
        });
        self.state.project_team_members.retain(|record| {
            record.project_id != project_id || record.member_person_id != member.person_id
        });
        self.record_share_audit(
            &account.person_id,
            "project_team_member_removed",
            Some(&member.person_id),
            serde_json::json!({
                "project_id": project_id,
                "member_person_id": member.person_id,
            }),
        );
        self.persist()?;
        Ok(team_member_view(&existing, &member.email))
    }

    pub fn project_team(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<ProjectTeam, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_reader(&account, project_id)?;
        let team = self
            .state
            .project_teams
            .iter()
            .find(|team| team.project_id == project_id)
            .cloned()
            .ok_or_else(|| StoreError::Validation("project has no team space".to_owned()))?;
        self.team_view(&team)
    }

    pub fn set_project_share_policy(
        &mut self,
        account_token: &str,
        project_id: &str,
        policy: &str,
    ) -> Result<ProjectSharePolicyRecord, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        let policy = ProjectSharePolicy::parse(policy)?;
        if let Some(record) = self
            .state
            .project_policies
            .iter_mut()
            .find(|record| record.project_id == project_id)
        {
            record.policy = policy.clone();
        } else {
            self.state.project_policies.push(ProjectPolicyRecord {
                project_id: project_id.to_owned(),
                policy: policy.clone(),
            });
        }
        self.persist()?;
        Ok(ProjectSharePolicyRecord {
            project_id: project_id.to_owned(),
            policy,
        })
    }

    pub fn share_candidates(
        &self,
        account_token: &str,
        project_id: &str,
        email: &str,
    ) -> Result<Vec<ProjectShareCandidate>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        let member = self.member_account(project_id, email)?;
        let policy = self.project_share_policy(project_id);
        let mut candidates = Vec::new();
        for memory in self.live_project_memories(project_id) {
            if !candidate_memory(&memory, project_id) {
                continue;
            }
            if self.share_is_active(project_id, &memory.memory_id, &member.person_id)
                || self.candidate_dismissed(project_id, &memory.memory_id, &member.person_id)
            {
                continue;
            }
            candidates.push(ProjectShareCandidate {
                memory_id: memory.memory_id.clone(),
                reason: "promoted project memory is not shared with the member".to_owned(),
                not_auto_shared_reason: not_auto_shared_reason(&policy, &memory),
            });
        }
        Ok(candidates)
    }

    pub fn dismiss_share_candidate(
        &mut self,
        account_token: &str,
        project_id: &str,
        memory_id: &str,
        email: &str,
    ) -> Result<ShareCandidateDismissal, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        let member = self.member_account(project_id, email)?;
        let memory = self
            .state
            .memories
            .iter()
            .find(|memory| {
                memory.memory_id == memory_id && memory.project_id.as_deref() == Some(project_id)
            })
            .cloned()
            .ok_or_else(|| {
                StoreError::Validation(
                    "personal observations and non-project memories cannot be shared".to_owned(),
                )
            })?;
        if memory.scope != MemoryScope::Project {
            return Err(StoreError::Validation(
                "personal observations and non-project memories cannot be shared".to_owned(),
            ));
        }
        if !self.candidate_dismissed(project_id, &memory.memory_id, &member.person_id) {
            self.state
                .dismissed_share_candidates
                .push(DismissedShareCandidate {
                    project_id: project_id.to_owned(),
                    memory_id: memory.memory_id.clone(),
                    member_person_id: member.person_id,
                });
            self.persist()?;
        }
        Ok(ShareCandidateDismissal {
            project_id: project_id.to_owned(),
            memory_id: memory.memory_id,
            dismissed: true,
        })
    }

    pub fn project_conflicts(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectConflict>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        validate_project_text(project_id, "project_id")?;
        let memories =
            if self.project_owner_id(project_id).as_deref() == Some(account.person_id.as_str()) {
                self.live_project_memories(project_id)
            } else if self.state.project_members.iter().any(|record| {
                record.project_id == project_id && record.member_person_id == account.person_id
            }) {
                self.shared_project_memories(account_token, project_id)?
                    .into_iter()
                    .map(|shared| shared.memory)
                    .collect()
            } else if self
                .state
                .projects
                .iter()
                .any(|project| project.project_id == project_id)
            {
                return Err(StoreError::Validation(
                    "account does not own the project".to_owned(),
                ));
            } else {
                return Err(StoreError::Validation(format!(
                    "unknown project: {project_id}"
                )));
            };
        Ok(competing_project_memories(&memories))
    }

    pub fn project_revocations(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectMemoryShare>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        validate_project_text(project_id, "project_id")?;
        if !self
            .state
            .projects
            .iter()
            .any(|project| project.project_id == project_id)
        {
            return Err(StoreError::Validation(format!(
                "unknown project: {project_id}"
            )));
        }
        let is_owner =
            self.project_owner_id(project_id).as_deref() == Some(account.person_id.as_str());
        let is_member = self.state.project_members.iter().any(|record| {
            record.project_id == project_id && record.member_person_id == account.person_id
        });
        if !is_owner && !is_member {
            return Err(StoreError::Validation(
                "account does not own the project".to_owned(),
            ));
        }
        Ok(self
            .state
            .project_shares
            .iter()
            .filter(|share| {
                share.project_id == project_id
                    && share.revoked_at.is_some()
                    && (is_owner || share.member_person_id == account.person_id)
            })
            .map(share_view)
            .collect())
    }

    fn project_owner_id(&self, project_id: &str) -> Option<String> {
        self.state
            .project_owners
            .iter()
            .find(|record| record.project_id == project_id)
            .map(|record| record.person_id.clone())
    }

    fn claim_project_owner(&mut self, person_id: &str, project_id: &str) {
        if self
            .state
            .project_owners
            .iter()
            .any(|record| record.project_id == project_id)
        {
            return;
        }
        self.state.project_owners.push(ProjectOwnerRecord {
            project_id: project_id.to_owned(),
            person_id: person_id.to_owned(),
        });
    }

    fn record_memory_owner(&mut self, person_id: &str, memory_id: &str) {
        if self
            .state
            .memory_owners
            .iter()
            .any(|record| record.memory_id == memory_id)
        {
            return;
        }
        self.state.memory_owners.push(MemoryOwnerRecord {
            memory_id: memory_id.to_owned(),
            person_id: person_id.to_owned(),
        });
    }

    fn memory_owned_by(&self, memory_id: &str, person_id: &str) -> bool {
        self.state
            .memory_owners
            .iter()
            .any(|record| record.memory_id == memory_id && record.person_id == person_id)
    }

    fn require_project_reader(
        &self,
        account: &Account,
        project_id: &str,
    ) -> Result<(), StoreError> {
        validate_project_text(project_id, "project_id")?;
        if !self
            .state
            .projects
            .iter()
            .any(|project| project.project_id == project_id)
        {
            return Err(StoreError::Validation(format!(
                "unknown project: {project_id}"
            )));
        }
        if self.project_owner_id(project_id).as_deref() == Some(account.person_id.as_str())
            || self.state.project_members.iter().any(|record| {
                record.project_id == project_id && record.member_person_id == account.person_id
            })
        {
            return Ok(());
        }
        Err(StoreError::Validation(
            "account is not a project member".to_owned(),
        ))
    }

    fn require_project_team(&self, project_id: &str) -> Result<(), StoreError> {
        if self
            .state
            .project_teams
            .iter()
            .any(|team| team.project_id == project_id)
        {
            return Ok(());
        }
        Err(StoreError::Validation(
            "project has no team space".to_owned(),
        ))
    }

    fn team_view(&self, team: &ProjectTeamRecord) -> Result<ProjectTeam, StoreError> {
        let mut members = Vec::new();
        for record in self
            .state
            .project_team_members
            .iter()
            .filter(|record| record.project_id == team.project_id)
        {
            let email = self
                .state
                .accounts
                .iter()
                .find(|account| account.account.person_id == record.member_person_id)
                .map(|account| account.account.email.clone())
                .ok_or_else(|| StoreError::Validation("unknown account email".to_owned()))?;
            members.push(team_member_view(record, &email));
        }
        members.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then(left.person_id.cmp(&right.person_id))
        });
        Ok(ProjectTeam {
            project_id: team.project_id.clone(),
            name: team.name.clone(),
            created_at: team.created_at.clone(),
            members,
        })
    }

    fn require_project_owner(&self, account: &Account, project_id: &str) -> Result<(), StoreError> {
        validate_project_text(project_id, "project_id")?;
        if !self
            .state
            .projects
            .iter()
            .any(|project| project.project_id == project_id)
        {
            return Err(StoreError::Validation(format!(
                "unknown project: {project_id}"
            )));
        }
        if self
            .project_owner_id(project_id)
            .is_some_and(|owner| owner != account.person_id)
        {
            return Err(StoreError::Validation(
                "account does not own the project".to_owned(),
            ));
        }
        Ok(())
    }

    fn account_by_email(&self, email: &str) -> Result<Account, StoreError> {
        self.state
            .accounts
            .iter()
            .find(|record| record.account.email == email)
            .map(|record| record.account.clone())
            .ok_or_else(|| StoreError::Validation("unknown account email".to_owned()))
    }

    fn shareable_project_memory(
        &self,
        project_id: &str,
        memory_id: &str,
    ) -> Result<Memory, StoreError> {
        let Some(memory) = self
            .state
            .memories
            .iter()
            .find(|memory| memory.memory_id == memory_id)
            .cloned()
        else {
            if self
                .state
                .observations
                .iter()
                .any(|observation| observation.event_id == memory_id)
            {
                return Err(StoreError::Validation(
                    "personal observations and non-project memories cannot be shared".to_owned(),
                ));
            }
            return Err(StoreError::Validation(format!(
                "unknown memory: {memory_id}"
            )));
        };
        if self.memory_is_forgotten(&memory) || is_expired(&memory.lifecycle) {
            return Err(StoreError::Validation(
                "forgotten or expired memories cannot be shared".to_owned(),
            ));
        }
        if memory.lifecycle.sensitivity == Sensitivity::Secret {
            return Err(StoreError::Validation(
                "secret memories cannot be shared".to_owned(),
            ));
        }
        if memory.scope == MemoryScope::Project && memory.project_id.as_deref() == Some(project_id)
        {
            return Ok(memory);
        }
        if memory.scope == MemoryScope::Project && memory.project_id.is_some() {
            return Err(StoreError::Validation(
                "memory does not belong to the project".to_owned(),
            ));
        }
        Err(StoreError::Validation(
            "personal observations and non-project memories cannot be shared".to_owned(),
        ))
    }

    fn live_project_memories(&self, project_id: &str) -> Vec<Memory> {
        let mut memories = self
            .state
            .memories
            .iter()
            .filter(|memory| {
                memory.project_id.as_deref() == Some(project_id)
                    && !self.memory_is_forgotten(memory)
                    && !is_expired(&memory.lifecycle)
                    && !self.state.memories.iter().any(|candidate| {
                        candidate.project_id.as_deref() == Some(project_id)
                            && candidate.supersedes.as_deref() == Some(memory.memory_id.as_str())
                            && !self.memory_is_forgotten(candidate)
                            && !is_expired(&candidate.lifecycle)
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        memories.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        memories
    }

    fn member_account(&self, project_id: &str, email: &str) -> Result<Account, StoreError> {
        let member = self.account_by_email(&normalize_email(email)?)?;
        if !self.state.project_members.iter().any(|record| {
            record.project_id == project_id && record.member_person_id == member.person_id
        }) {
            return Err(StoreError::Validation(
                "account is not a project member".to_owned(),
            ));
        }
        Ok(member)
    }

    fn project_share_policy(&self, project_id: &str) -> ProjectSharePolicy {
        self.state
            .project_policies
            .iter()
            .find(|record| record.project_id == project_id)
            .map(|record| record.policy.clone())
            .unwrap_or_default()
    }

    fn share_is_active(&self, project_id: &str, memory_id: &str, member_person_id: &str) -> bool {
        self.state.project_shares.iter().any(|share| {
            share.project_id == project_id
                && share.memory_id == memory_id
                && share.member_person_id == member_person_id
                && share.revoked_at.is_none()
        })
    }

    fn candidate_dismissed(
        &self,
        project_id: &str,
        memory_id: &str,
        member_person_id: &str,
    ) -> bool {
        self.state.dismissed_share_candidates.iter().any(|record| {
            record.project_id == project_id
                && record.memory_id == memory_id
                && record.member_person_id == member_person_id
        })
    }

    fn grant_project_share(
        &mut self,
        owner_person_id: &str,
        project_id: &str,
        memory_id: &str,
        member_person_id: &str,
        share_reason: &str,
    ) -> ProjectMemoryShare {
        if let Some(existing) = self.state.project_shares.iter().find(|share| {
            share.project_id == project_id
                && share.memory_id == memory_id
                && share.member_person_id == member_person_id
                && share.revoked_at.is_none()
        }) {
            return share_view(existing);
        }
        let reactivated = self
            .state
            .project_shares
            .iter_mut()
            .find(|share| {
                share.project_id == project_id
                    && share.memory_id == memory_id
                    && share.member_person_id == member_person_id
            })
            .map(|share| {
                share.revoked_at = None;
                share.share_reason = Some(share_reason.to_owned());
                share_view(share)
            });
        if let Some(view) = reactivated {
            self.record_share_audit(
                owner_person_id,
                "project_memory_shared",
                Some(&view.memory_id),
                share_audit_metadata(project_id, member_person_id, &view.memory_id),
            );
            return view;
        }
        let record = ProjectShareRecord {
            project_id: project_id.to_owned(),
            memory_id: memory_id.to_owned(),
            owner_person_id: owner_person_id.to_owned(),
            member_person_id: member_person_id.to_owned(),
            created_at: timestamp(),
            revoked_at: None,
            share_reason: Some(share_reason.to_owned()),
        };
        self.state.project_shares.push(record.clone());
        self.record_share_audit(
            owner_person_id,
            "project_memory_shared",
            Some(memory_id),
            share_audit_metadata(project_id, member_person_id, memory_id),
        );
        share_view(&record)
    }

    fn auto_share_new_project_memory(
        &mut self,
        owner_person_id: &str,
        project_id: &str,
        memory: &Memory,
    ) {
        if self.project_owner_id(project_id).as_deref() != Some(owner_person_id) {
            return;
        }
        if self.project_share_policy(project_id)
            != ProjectSharePolicy::AutoNonsensitiveProjectMemory
        {
            return;
        }
        if memory.scope != MemoryScope::Project
            || matches!(
                memory.lifecycle.sensitivity,
                Sensitivity::Secret | Sensitivity::Sensitive
            )
        {
            return;
        }
        let members = self
            .state
            .project_members
            .iter()
            .filter(|record| record.project_id == project_id)
            .map(|record| record.member_person_id.clone())
            .collect::<Vec<_>>();
        let memory_id = memory.memory_id.clone();
        for member_person_id in members {
            if self.candidate_dismissed(project_id, &memory_id, &member_person_id) {
                continue;
            }
            self.grant_project_share(
                owner_person_id,
                project_id,
                &memory_id,
                &member_person_id,
                SHARE_REASON_AUTO,
            );
        }
    }

    fn reject_member_mutation(&self, account: &Account, target_id: &str) -> Result<(), StoreError> {
        let Some(owner) = self
            .state
            .memory_owners
            .iter()
            .find(|record| record.memory_id == target_id)
        else {
            return Ok(());
        };
        if owner.person_id == account.person_id {
            return Ok(());
        }
        if self.state.project_shares.iter().any(|share| {
            share.memory_id == target_id && share.member_person_id == account.person_id
        }) {
            return Err(StoreError::Validation(
                "account does not own the memory".to_owned(),
            ));
        }
        Err(StoreError::Validation(format!(
            "unknown memory or event: {target_id}"
        )))
    }

    fn record_share_audit(
        &mut self,
        person_id: &str,
        action: &str,
        subject_id: Option<&str>,
        metadata: serde_json::Value,
    ) {
        self.state.audit_events.push(StoredAuditEvent {
            person_id: person_id.to_owned(),
            audit_id: format!("audit-{}", &hash_token(&random_token("audit"))[..24]),
            action: action.to_owned(),
            subject_id: subject_id.map(str::to_owned),
            actor_device_id: None,
            metadata,
            created_at: timestamp(),
        });
    }

    fn issue_auth_session(&mut self, account: Account) -> Result<AuthSessionResult, StoreError> {
        let token = random_token("cairn-session");
        let token_hash = hash_token(&token);
        let expires_at = (now_seconds() + AUTH_SESSION_SECONDS).to_string();
        let session_id = format!("auth-{}", &token_hash[..16]);
        self.state.auth_sessions.push(AuthSessionRecord {
            session_id,
            person_id: account.person_id.clone(),
            token_hash,
            created_at: timestamp(),
            expires_at: expires_at.clone(),
            revoked_at: None,
        });
        Ok(AuthSessionResult {
            account,
            token,
            expires_at,
        })
    }

    pub fn register_device(
        &mut self,
        device_id: impl Into<String>,
        person_id: impl Into<String>,
        agent_id: Option<String>,
        name: impl Into<String>,
    ) -> Result<DeviceRegistration, StoreError> {
        let device_id = device_id.into();
        let person_id = person_id.into();
        let name = name.into();
        for (field, value) in [
            ("device_id", device_id.as_str()),
            ("person_id", person_id.as_str()),
            ("device name", name.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(StoreError::Validation(format!("{field} must not be empty")));
            }
        }
        if let Some(existing) = self
            .state
            .devices
            .iter()
            .find(|record| record.device.device_id == device_id)
        {
            if existing.device.person_id != person_id {
                return Err(StoreError::Validation(format!(
                    "device already belongs to another person: {device_id}"
                )));
            }
            return Ok(DeviceRegistration {
                device: existing.device.clone(),
                token: existing.token.clone(),
            });
        }

        let now = timestamp();
        let device = Device {
            device_id,
            person_id,
            agent_id,
            name,
            status: DeviceStatus::Active,
            registered_at: now.clone(),
            last_seen_at: now,
            cursor: 0,
        };
        let token = device_token(&device.device_id);
        self.state.devices.push(DeviceRecord {
            device: device.clone(),
            token: token.clone(),
        });
        self.persist()?;
        Ok(DeviceRegistration { device, token })
    }

    pub fn devices(&self) -> impl Iterator<Item = &Device> {
        self.state.devices.iter().map(|record| &record.device)
    }

    pub fn authenticate_device(&mut self, token: &str) -> Result<Device, StoreError> {
        let record = self
            .state
            .devices
            .iter_mut()
            .find(|record| record.token == token)
            .ok_or_else(|| StoreError::Validation("device authentication failed".to_owned()))?;
        if record.device.status != DeviceStatus::Active {
            return Err(StoreError::Validation("device is revoked".to_owned()));
        }
        record.device.last_seen_at = timestamp();
        let device = record.device.clone();
        self.persist()?;
        Ok(device)
    }

    pub fn revoke_device(
        &mut self,
        auth_token: &str,
        target_device_id: &str,
    ) -> Result<Device, StoreError> {
        let caller = self.authenticate_device(auth_token)?;
        let target = self
            .state
            .devices
            .iter_mut()
            .find(|record| record.device.device_id == target_device_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown device: {target_device_id}")))?;
        if caller.person_id != target.device.person_id {
            return Err(StoreError::Validation(
                "device is outside the caller scope".to_owned(),
            ));
        }
        target.device.status = DeviceStatus::Revoked;
        target.device.last_seen_at = timestamp();
        let device = target.device.clone();
        self.persist()?;
        Ok(device)
    }

    pub fn rotate_device_token(
        &mut self,
        auth_token: &str,
        target_device_id: &str,
    ) -> Result<DeviceRegistration, StoreError> {
        let caller = self.authenticate_device(auth_token)?;
        let target = self
            .state
            .devices
            .iter_mut()
            .find(|record| record.device.device_id == target_device_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown device: {target_device_id}")))?;
        if caller.person_id != target.device.person_id {
            return Err(StoreError::Validation(
                "device is outside the caller scope".to_owned(),
            ));
        }
        let token = device_token(&target.device.device_id);
        target.token = token.clone();
        target.device.last_seen_at = timestamp();
        let device = target.device.clone();
        self.persist()?;
        Ok(DeviceRegistration { device, token })
    }

    pub fn sync_push(
        &mut self,
        auth_token: &str,
        input: ObservationInput,
    ) -> Result<SyncPushResult, StoreError> {
        let device = self.authenticate_device(auth_token)?;
        if let Some(existing) = self
            .state
            .observations
            .iter()
            .find(|observation| observation.event_id == input.event_id)
            .cloned()
        {
            let sync_event = self
                .state
                .sync_events
                .iter()
                .find(|event| event.event_id == input.event_id)
                .ok_or_else(|| {
                    StoreError::Validation(format!(
                        "event already exists outside sync scope: {}",
                        input.event_id
                    ))
                })?;
            if sync_event.person_id != device.person_id {
                return Err(StoreError::Validation(format!(
                    "event belongs to another person: {}",
                    input.event_id
                )));
            }
            return Ok(SyncPushResult {
                accepted: true,
                duplicate: true,
                cursor: sync_event.cursor,
                observation: existing,
            });
        }

        let capture = self.capture_observation(input)?;
        self.state.next_sync_cursor = self.state.next_sync_cursor.saturating_add(1);
        let cursor = self.state.next_sync_cursor;
        self.state.sync_events.push(SyncEvent {
            cursor,
            event_id: capture.observation.event_id.clone(),
            person_id: device.person_id,
            device_id: device.device_id,
            observation: capture.observation.clone(),
        });
        self.persist()?;
        Ok(SyncPushResult {
            accepted: true,
            duplicate: false,
            cursor,
            observation: capture.observation,
        })
    }

    pub fn sync_pull(
        &mut self,
        auth_token: &str,
        cursor: u64,
    ) -> Result<SyncPullResult, StoreError> {
        let device = self.authenticate_device(auth_token)?;
        let events: Vec<SyncEvent> = self
            .state
            .sync_events
            .iter()
            .filter(|event| event.person_id == device.person_id && event.cursor > cursor)
            .cloned()
            .collect();
        let next_cursor = events
            .last()
            .map(|event| event.cursor)
            .unwrap_or(cursor.max(device.cursor));
        if let Some(record) = self
            .state
            .devices
            .iter_mut()
            .find(|record| record.device.device_id == device.device_id)
        {
            record.device.cursor = record.device.cursor.max(next_cursor);
        }
        self.persist()?;
        Ok(SyncPullResult {
            cursor: next_cursor,
            events,
        })
    }

    pub fn sync_diagnostics(&mut self, auth_token: &str) -> Result<SyncDiagnostics, StoreError> {
        let device = self.authenticate_device(auth_token)?;
        let server_cursor = self
            .state
            .sync_events
            .iter()
            .filter(|event| event.person_id == device.person_id)
            .map(|event| event.cursor)
            .max()
            .unwrap_or(0);
        let lag = server_cursor.saturating_sub(device.cursor);
        Ok(SyncDiagnostics {
            device_id: device.device_id,
            device_cursor: device.cursor,
            server_cursor,
            lag,
            pending_events: 0,
            rejected_events: 0,
            status: if lag == 0 { "caught_up" } else { "behind" }.to_owned(),
            recovery_hint: (lag > 0).then(|| {
                "Pull from the last persisted cursor to resume synchronization.".to_owned()
            }),
        })
    }

    pub fn capture_observation(
        &mut self,
        input: ObservationInput,
    ) -> Result<CaptureResult, StoreError> {
        validate_observation(&input)?;

        if let Some(existing) = self
            .state
            .observations
            .iter()
            .find(|observation| observation.event_id == input.event_id)
        {
            return Ok(CaptureResult {
                accepted: true,
                duplicate: true,
                observation: existing.clone(),
            });
        }

        let context = if input.context == WorkContext::default() {
            self.state
                .sessions
                .iter()
                .find(|session| session.session_id == input.session_id)
                .map(|session| session.context.clone())
                .unwrap_or_default()
        } else {
            input.context.clone()
        };
        let mut normalized_input = input.clone();
        normalized_input.context = context;
        let observation = normalize_observation(&normalized_input)?;
        self.state.observations.push(observation.clone());
        self.persist()?;

        Ok(CaptureResult {
            accepted: true,
            duplicate: false,
            observation,
        })
    }

    pub fn end_session(&mut self, session_id: &str) -> Result<Memory, StoreError> {
        if session_id.trim().is_empty() {
            return Err(StoreError::Validation(
                "session_id must not be empty".to_owned(),
            ));
        }

        let session = self
            .state
            .sessions
            .iter_mut()
            .find(|session| session.session_id == session_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown session: {session_id}")))?;
        if session.ended_at.is_none() {
            session.ended_at = Some(timestamp());
        }

        let observations: Vec<Observation> = self
            .state
            .observations
            .iter()
            .filter(|observation| {
                observation.session_id == session_id
                    && !self.state.forgotten_ids.contains(&observation.event_id)
                    && !is_expired(&observation.lifecycle)
            })
            .cloned()
            .collect();

        if observations.is_empty() {
            return Err(StoreError::Validation(format!(
                "session has no retained observations: {session_id}"
            )));
        }

        let memory_id = format!("memory-{session_id}");
        if let Some(memory) = self
            .state
            .memories
            .iter()
            .find(|memory| memory.memory_id == memory_id)
        {
            self.persist()?;
            return Ok(memory.clone());
        }

        let memory = derive_session_memory(session_id, &session.context, &observations)?;
        self.state.memories.push(memory.clone());
        self.persist()?;
        Ok(memory)
    }

    pub fn remember(
        &mut self,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
    ) -> Result<Memory, StoreError> {
        self.remember_with_lifecycle(content, concepts, context, LifecycleInput::default())
    }

    pub fn remember_with_lifecycle(
        &mut self,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
        lifecycle: LifecycleInput,
    ) -> Result<Memory, StoreError> {
        self.remember_with_kind(content, concepts, context, lifecycle, MemoryKind::Explicit)
    }

    pub fn remember_lesson(
        &mut self,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
        lifecycle: LifecycleInput,
    ) -> Result<Memory, StoreError> {
        self.remember_with_kind(content, concepts, context, lifecycle, MemoryKind::Lesson)
    }

    pub fn remember_with_kind(
        &mut self,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
        lifecycle: LifecycleInput,
        kind: MemoryKind,
    ) -> Result<Memory, StoreError> {
        if content.trim().is_empty() {
            return Err(StoreError::Validation(
                "content must not be empty".to_owned(),
            ));
        }
        if content.len() > MAX_OBSERVATION_BYTES {
            return Err(StoreError::Validation(format!(
                "content exceeds the {} byte limit",
                MAX_OBSERVATION_BYTES
            )));
        }
        validate_lifecycle_input(&lifecycle)?;

        let memory = build_explicit_memory_record(content, concepts, context, lifecycle, kind)?;
        self.state.memories.push(memory.clone());
        self.persist()?;
        Ok(memory)
    }

    pub fn remember_for_account(
        &mut self,
        account_token: &str,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
        lifecycle: LifecycleInput,
        kind: MemoryKind,
    ) -> Result<Memory, StoreError> {
        let account = self.authenticate_account(account_token)?;
        let memory = self.remember_with_kind(content, concepts, context, lifecycle, kind)?;
        self.record_memory_owner(&account.person_id, &memory.memory_id);
        self.persist()?;
        Ok(memory)
    }

    pub fn correct(
        &mut self,
        target_id: &str,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
        lifecycle: LifecycleInput,
    ) -> Result<Memory, StoreError> {
        if target_id.trim().is_empty() {
            return Err(StoreError::Validation(
                "target_id must not be empty".to_owned(),
            ));
        }
        if content.trim().is_empty() {
            return Err(StoreError::Validation(
                "content must not be empty".to_owned(),
            ));
        }
        if content.len() > MAX_OBSERVATION_BYTES {
            return Err(StoreError::Validation(format!(
                "content exceeds the {} byte limit",
                MAX_OBSERVATION_BYTES
            )));
        }
        validate_lifecycle_input(&lifecycle)?;
        let target = self
            .state
            .memories
            .iter()
            .find(|memory| memory.memory_id == target_id)
            .map(|memory| {
                (
                    memory.memory_id.clone(),
                    memory.context.clone(),
                    memory.scope.clone(),
                    memory.project_id.clone(),
                )
            })
            .or_else(|| {
                self.state
                    .observations
                    .iter()
                    .find(|observation| observation.event_id == target_id)
                    .map(|observation| {
                        (
                            observation.event_id.clone(),
                            observation.context.clone(),
                            memory_scope(&observation.context),
                            None,
                        )
                    })
            })
            .ok_or_else(|| {
                StoreError::Validation(format!("unknown memory or event: {target_id}"))
            })?;

        let suffix = timestamp_nanos();
        let correction_event = format!("event-correction-{suffix}");
        let mut lifecycle = Lifecycle::explicit(&lifecycle, content);
        lifecycle.confidence = 100;
        let memory = Memory {
            memory_id: format!("memory-correction-{suffix}"),
            session_id: format!("session-correction-{suffix}"),
            content: build_explicit_memory(content, concepts),
            source_event_ids: vec![correction_event],
            context: if context == WorkContext::default() {
                target.1
            } else {
                context.clone()
            },
            lifecycle,
            kind: MemoryKind::Correction,
            scope: if context == WorkContext::default() {
                target.2
            } else {
                memory_scope(&context)
            },
            project_id: target.3,
            next_steps: Vec::new(),
            unresolved_work: Vec::new(),
            supersedes: Some(target.0),
            promoted_from: None,
            promotion_reason: None,
            verified_commit: None,
            created_at: timestamp(),
        };
        self.state.memories.push(memory.clone());
        self.persist()?;
        Ok(memory)
    }

    pub fn correct_for_account(
        &mut self,
        account_token: &str,
        target_id: &str,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
        lifecycle: LifecycleInput,
    ) -> Result<Memory, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.reject_member_mutation(&account, target_id)?;
        let memory = self.correct(target_id, content, concepts, context, lifecycle)?;
        self.record_memory_owner(&account.person_id, &memory.memory_id);
        self.persist()?;
        Ok(memory)
    }

    pub fn export(&self) -> ExportBundle {
        let mut forgotten_ids: Vec<String> = self.state.forgotten_ids.iter().cloned().collect();
        forgotten_ids.sort();
        ExportBundle {
            format: EXPORT_FORMAT.to_owned(),
            version: EXPORT_VERSION,
            exported_at: timestamp(),
            sessions: self.state.sessions.clone(),
            observations: self.state.observations.clone(),
            memories: self.state.memories.clone(),
            forgotten_ids,
        }
    }

    pub fn export_json(&self) -> Result<String, StoreError> {
        Ok(serde_json::to_string_pretty(&self.export())?)
    }

    pub fn import_json(&mut self, raw: &str) -> Result<ImportResult, StoreError> {
        let bundle: ExportBundle = serde_json::from_str(raw)?;
        self.import_bundle(bundle)
    }

    pub fn import_bundle(&mut self, mut bundle: ExportBundle) -> Result<ImportResult, StoreError> {
        normalize_export_bundle(&mut bundle)?;
        let mut candidate = self.state.clone();
        let mut result = ImportResult {
            imported_sessions: 0,
            imported_observations: 0,
            imported_memories: 0,
            imported_forgotten_ids: 0,
        };

        for session in bundle.sessions {
            if let Some(existing) = candidate
                .sessions
                .iter()
                .find(|existing| existing.session_id == session.session_id)
            {
                if existing != &session {
                    return Err(import_conflict("session", &session.session_id));
                }
            } else {
                candidate.sessions.push(session);
                result.imported_sessions += 1;
            }
        }

        for observation in bundle.observations {
            if let Some(existing) = candidate
                .observations
                .iter()
                .find(|existing| existing.event_id == observation.event_id)
            {
                if existing != &observation {
                    return Err(import_conflict("observation", &observation.event_id));
                }
            } else {
                candidate.observations.push(observation);
                result.imported_observations += 1;
            }
        }

        for memory in bundle.memories {
            if let Some(existing) = candidate
                .memories
                .iter()
                .find(|existing| existing.memory_id == memory.memory_id)
            {
                if existing != &memory {
                    return Err(import_conflict("memory", &memory.memory_id));
                }
            } else {
                candidate.memories.push(memory);
                result.imported_memories += 1;
            }
        }

        for forgotten_id in bundle.forgotten_ids {
            if candidate.forgotten_ids.contains(&forgotten_id) {
                continue;
            }
            candidate.forgotten_ids.insert(forgotten_id);
            result.imported_forgotten_ids += 1;
        }

        if result.imported_sessions > 0
            || result.imported_observations > 0
            || result.imported_memories > 0
            || result.imported_forgotten_ids > 0
        {
            self.state = candidate;
            self.persist()?;
            self.sweep_expired()?;
        }
        Ok(result)
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<RecallResult>, StoreError> {
        self.search_with_filter(query, limit, &RecallFilter::default())
    }

    pub fn search_with_filter(
        &self,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<Vec<RecallResult>, StoreError> {
        if query.trim().is_empty() {
            return Err(StoreError::Validation("query must not be empty".to_owned()));
        }

        let terms = tokenize(query);
        let mut results: Vec<RecallResult> = self
            .state
            .memories
            .iter()
            .filter(|memory| self.memory_is_recalled(memory))
            .filter(|memory| memory_matches_filter(memory, filter))
            .filter_map(|memory| {
                let score = score_memory(memory, &terms);
                (score > 0.0).then(|| {
                    recall_result(
                        memory,
                        score,
                        &self.state.memories,
                        &self.state.forgotten_ids,
                        filter,
                    )
                })
            })
            .collect();

        results.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.memory_id.cmp(&right.memory_id))
        });
        results.truncate(limit);
        Ok(results)
    }

    pub fn context(&self, query: &str, limit: usize) -> Result<ContextPack, StoreError> {
        self.context_with_filter(query, limit, &RecallFilter::default())
    }

    pub fn context_with_filter(
        &self,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, StoreError> {
        let mut results = self.search_with_filter(query, usize::MAX, filter)?;
        Ok(build_context_pack(query.to_owned(), &mut results, limit))
    }

    pub fn search_for_account(
        &self,
        account_token: &str,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<Vec<RecallResult>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        let results = self.search_with_filter(query, limit, filter)?;
        Ok(results
            .into_iter()
            .filter(|result| self.memory_owned_by(&result.memory_id, &account.person_id))
            .collect())
    }

    pub fn context_for_account(
        &self,
        account_token: &str,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, StoreError> {
        let mut results = self.search_for_account(account_token, query, usize::MAX, filter)?;
        Ok(build_context_pack(query.to_owned(), &mut results, limit))
    }

    pub fn resume_context(
        &self,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, StoreError> {
        let mut results: Vec<RecallResult> = self
            .state
            .memories
            .iter()
            .rev()
            .filter(|memory| memory.kind == MemoryKind::SessionSummary)
            .filter(|memory| self.memory_is_recalled(memory))
            .filter(|memory| memory_matches_filter(memory, filter))
            .map(|memory| {
                recall_result(
                    memory,
                    1.0,
                    &self.state.memories,
                    &self.state.forgotten_ids,
                    filter,
                )
            })
            .collect();
        Ok(build_context_pack("resume".to_owned(), &mut results, limit))
    }

    pub fn unresolved_context(
        &self,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, StoreError> {
        let mut results: Vec<RecallResult> = self
            .state
            .memories
            .iter()
            .rev()
            .filter(|memory| memory.kind == MemoryKind::SessionSummary)
            .filter(|memory| !memory.unresolved_work.is_empty())
            .filter(|memory| self.memory_is_recalled(memory))
            .filter(|memory| memory_matches_filter(memory, filter))
            .map(|memory| {
                recall_result(
                    memory,
                    1.0,
                    &self.state.memories,
                    &self.state.forgotten_ids,
                    filter,
                )
            })
            .collect();
        Ok(build_context_pack(
            "unresolved".to_owned(),
            &mut results,
            limit,
        ))
    }

    pub fn why_changed_context(
        &self,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, StoreError> {
        let mut results = self.search_with_filter(query, usize::MAX, filter)?;
        results.sort_by(|left, right| {
            right
                .decision_reason
                .is_some()
                .cmp(&left.decision_reason.is_some())
                .then_with(|| right.relationships.len().cmp(&left.relationships.len()))
                .then_with(|| {
                    right
                        .score
                        .partial_cmp(&left.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        Ok(build_context_pack(
            format!("why changed: {query}"),
            &mut results,
            limit,
        ))
    }

    fn memory_is_forgotten(&self, memory: &Memory) -> bool {
        self.state.forgotten_ids.contains(&memory.memory_id)
            || memory
                .source_event_ids
                .iter()
                .any(|event_id| self.state.forgotten_ids.contains(event_id))
    }

    fn memory_is_superseded(&self, memory: &Memory) -> bool {
        self.state.memories.iter().any(|candidate| {
            candidate.supersedes.as_deref() == Some(memory.memory_id.as_str())
                && !self.memory_is_forgotten(candidate)
                && !is_expired(&candidate.lifecycle)
        })
    }

    fn memory_is_recalled(&self, memory: &Memory) -> bool {
        !self.memory_is_forgotten(memory)
            && !is_expired(&memory.lifecycle)
            && !self.memory_is_superseded(memory)
    }

    fn persist(&self) -> Result<(), StoreError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let serialized = serde_json::to_vec_pretty(&self.state)?;
        let temporary_path = self.path.with_extension("json.tmp");
        fs::write(&temporary_path, serialized)?;
        fs::rename(temporary_path, &self.path)?;
        Ok(())
    }
}

pub fn build_context_pack(
    query: String,
    results: &mut Vec<RecallResult>,
    limit: usize,
) -> ContextPack {
    let omitted_count = results.len().saturating_sub(limit);
    let omitted_memory_ids = results
        .iter()
        .skip(limit)
        .map(|result| result.memory_id.clone())
        .collect::<Vec<_>>();
    results.truncate(limit);
    ContextPack {
        query,
        omitted_count,
        recovery_hint: (omitted_count > 0).then(|| {
            "Increase the limit or query one of the omitted memory IDs to recover additional context."
                .to_owned()
        }),
        omitted_memory_ids,
        results: std::mem::take(results),
    }
}

impl MemoryStore {
    pub fn forget(&mut self, id: &str) -> Result<(), StoreError> {
        self.hide_id(id)?;
        self.persist()
    }

    pub fn forget_for_account(&mut self, account_token: &str, id: &str) -> Result<(), StoreError> {
        self.hide_for_account(account_token, id, "memory_forget")
    }

    pub fn delete_for_account(&mut self, account_token: &str, id: &str) -> Result<(), StoreError> {
        self.hide_for_account(account_token, id, "memory_delete_retained")
    }

    fn hide_for_account(
        &mut self,
        account_token: &str,
        id: &str,
        action: &str,
    ) -> Result<(), StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.reject_member_mutation(&account, id)?;
        self.hide_id(id)?;
        self.record_share_audit(&account.person_id, action, Some(id), serde_json::json!({}));
        self.persist()
    }

    fn hide_id(&mut self, id: &str) -> Result<(), StoreError> {
        if id.trim().is_empty() {
            return Err(StoreError::Validation("id must not be empty".to_owned()));
        }
        let exists = self
            .state
            .observations
            .iter()
            .any(|item| item.event_id == id)
            || self.state.memories.iter().any(|item| item.memory_id == id);
        if !exists {
            return Err(StoreError::Validation(format!(
                "unknown memory or event: {id}"
            )));
        }
        self.state.forgotten_ids.insert(id.to_owned());
        if let Some(memory) = self
            .state
            .memories
            .iter()
            .find(|memory| memory.memory_id == id)
        {
            for event_id in &memory.source_event_ids {
                self.state.forgotten_ids.insert(event_id.clone());
            }
        }
        Ok(())
    }

    pub fn lifecycle_summary(&self) -> LifecycleSummary {
        LifecycleSummary {
            temporary_observations: self
                .state
                .observations
                .iter()
                .filter(|observation| observation.lifecycle.retention == Retention::Temporary)
                .count(),
            durable_memories: self
                .state
                .memories
                .iter()
                .filter(|memory| memory.lifecycle.retention == Retention::Durable)
                .count(),
            sensitive_items: self
                .state
                .observations
                .iter()
                .filter(|observation| sensitivity_rank(&observation.lifecycle.sensitivity) >= 2)
                .count()
                + self
                    .state
                    .memories
                    .iter()
                    .filter(|memory| sensitivity_rank(&memory.lifecycle.sensitivity) >= 2)
                    .count(),
            expired_items: self
                .state
                .observations
                .iter()
                .filter(|observation| is_expired(&observation.lifecycle))
                .count()
                + self
                    .state
                    .memories
                    .iter()
                    .filter(|memory| is_expired(&memory.lifecycle))
                    .count(),
        }
    }

    pub fn observations(&self) -> impl Iterator<Item = &Observation> {
        self.state.observations.iter()
    }

    pub fn sessions(&self) -> impl Iterator<Item = &Session> {
        self.state.sessions.iter()
    }
}

pub fn normalize_observation(input: &ObservationInput) -> Result<Observation, StoreError> {
    validate_observation(input)?;
    let lifecycle = Lifecycle::observation(
        &input.lifecycle,
        input.importance.as_deref(),
        &input.content,
    );
    Ok(Observation {
        event_id: input.event_id.clone(),
        session_id: input.session_id.clone(),
        content: redact_sensitive_content(&input.content),
        role: input.role.clone(),
        source: input.source.clone(),
        importance: input.importance.clone(),
        lifecycle,
        scope: MemoryScope::Session,
        context: input.context.clone(),
        captured_at: timestamp(),
    })
}

fn validate_observation(input: &ObservationInput) -> Result<(), StoreError> {
    for (name, value) in [
        ("event_id", input.event_id.as_str()),
        ("session_id", input.session_id.as_str()),
        ("content", input.content.as_str()),
        ("role", input.role.as_str()),
        ("source", input.source.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(StoreError::Validation(format!("{name} must not be empty")));
        }
    }
    if input.content.len() > MAX_OBSERVATION_BYTES {
        return Err(StoreError::Validation(format!(
            "content exceeds the {} byte limit",
            MAX_OBSERVATION_BYTES
        )));
    }
    validate_lifecycle_input(&input.lifecycle)?;
    Ok(())
}

fn validate_lifecycle_input(input: &LifecycleInput) -> Result<(), StoreError> {
    for (name, value) in [
        ("importance", input.importance),
        ("confidence", input.confidence),
        ("freshness", input.freshness),
    ] {
        if let Some(value) = value
            && value > 100
        {
            return Err(StoreError::Validation(format!(
                "lifecycle {name} must be between 0 and 100"
            )));
        }
    }
    if let Some(expires_at) = &input.expires_at {
        validate_timestamp(expires_at, "lifecycle expires_at")?;
    }
    Ok(())
}

fn validate_lifecycle(lifecycle: &Lifecycle) -> Result<(), StoreError> {
    for (name, value) in [
        ("importance", lifecycle.importance),
        ("confidence", lifecycle.confidence),
        ("freshness", lifecycle.freshness),
    ] {
        if value > 100 {
            return Err(StoreError::Validation(format!(
                "lifecycle {name} must be between 0 and 100"
            )));
        }
    }
    if let Some(expires_at) = &lifecycle.expires_at {
        validate_timestamp(expires_at, "lifecycle expires_at")?;
    }
    Ok(())
}

fn validate_timestamp(value: &str, field: &str) -> Result<(), StoreError> {
    value
        .parse::<u64>()
        .map(|_| ())
        .map_err(|_| StoreError::Validation(format!("{field} must be a Unix timestamp in seconds")))
}

fn validate_project_text(value: &str, field: &str) -> Result<(), StoreError> {
    if value.trim().is_empty() {
        return Err(StoreError::Validation(format!("{field} must not be empty")));
    }
    if value.len() > 512 {
        return Err(StoreError::Validation(format!(
            "{field} exceeds the 512 byte limit"
        )));
    }
    Ok(())
}

fn validate_entity_kind(kind: &str) -> Result<(), StoreError> {
    match kind.trim() {
        "file" | "symbol" | "decision" | "handoff" | "runbook" => Ok(()),
        _ => Err(StoreError::Validation(
            "entity kind must be file, symbol, decision, handoff, or runbook".to_owned(),
        )),
    }
}

fn importance_score(value: &str) -> u8 {
    match value.trim().to_ascii_lowercase().as_str() {
        "critical" => 100,
        "high" => 80,
        "medium" => 50,
        "low" => 20,
        _ => 50,
    }
}

fn classify_sensitivity(content: &str) -> Sensitivity {
    let lower = content.to_ascii_lowercase();
    if [
        "authorization: bearer ",
        "api_key=",
        "token=",
        "password=",
        "sk-",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        Sensitivity::Secret
    } else if ["private", "confidential", "internal-only", "personal data"]
        .iter()
        .any(|marker| lower.contains(marker))
    {
        Sensitivity::Sensitive
    } else {
        Sensitivity::Private
    }
}

fn sensitivity_rank(value: &Sensitivity) -> u8 {
    match value {
        Sensitivity::Public => 0,
        Sensitivity::Private => 1,
        Sensitivity::Sensitive => 2,
        Sensitivity::Secret => 3,
    }
}

fn is_expired(lifecycle: &Lifecycle) -> bool {
    lifecycle
        .expires_at
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|expires_at| expires_at <= now_seconds())
}

pub fn normalize_export_bundle(bundle: &mut ExportBundle) -> Result<(), StoreError> {
    if bundle.format != EXPORT_FORMAT {
        return Err(StoreError::Validation(format!(
            "unsupported export format {}; expected {EXPORT_FORMAT}",
            bundle.format
        )));
    }
    if bundle.version != EXPORT_VERSION {
        return Err(StoreError::Validation(format!(
            "unsupported export version {}; expected {EXPORT_VERSION}",
            bundle.version
        )));
    }
    for session in &bundle.sessions {
        if session.session_id.trim().is_empty() {
            return Err(StoreError::Validation(
                "export session_id must not be empty".to_owned(),
            ));
        }
    }
    for observation in &mut bundle.observations {
        if observation.event_id.trim().is_empty() || observation.session_id.trim().is_empty() {
            return Err(StoreError::Validation(
                "export observation ids must not be empty".to_owned(),
            ));
        }
        if observation.content.len() > MAX_OBSERVATION_BYTES {
            return Err(StoreError::Validation(format!(
                "export observation exceeds the {} byte limit",
                MAX_OBSERVATION_BYTES
            )));
        }
        observation.content = redact_sensitive_content(&observation.content);
        validate_lifecycle(&observation.lifecycle)?;
    }
    for memory in &mut bundle.memories {
        if memory.memory_id.trim().is_empty() || memory.session_id.trim().is_empty() {
            return Err(StoreError::Validation(
                "export memory ids must not be empty".to_owned(),
            ));
        }
        if memory.content.len() > MAX_MEMORY_BYTES {
            return Err(StoreError::Validation(format!(
                "export memory exceeds the {} byte limit",
                MAX_MEMORY_BYTES
            )));
        }
        memory.content = redact_sensitive_content(&memory.content);
        validate_lifecycle(&memory.lifecycle)?;
    }
    bundle.forgotten_ids.retain(|id| !id.trim().is_empty());
    bundle.forgotten_ids.sort();
    bundle.forgotten_ids.dedup();
    Ok(())
}

fn import_conflict(kind: &str, id: &str) -> StoreError {
    StoreError::Validation(format!("conflicting {kind} during import: {id}"))
}

fn timestamp() -> String {
    now_seconds().to_string()
}

pub fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn timestamp_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

pub fn normalize_email(email: &str) -> Result<String, StoreError> {
    let normalized = email.trim().to_ascii_lowercase();
    let valid = normalized.len() <= 320
        && normalized.split('@').count() == 2
        && normalized
            .split('@')
            .all(|part| !part.is_empty() && !part.chars().any(char::is_whitespace));
    if !valid {
        return Err(StoreError::Validation(
            "email must be a valid account identifier".to_owned(),
        ));
    }
    Ok(normalized)
}

pub fn validate_password(password: &str) -> Result<(), StoreError> {
    let length = password.len();
    if !(MIN_PASSWORD_BYTES..=1024).contains(&length) {
        return Err(StoreError::Validation(format!(
            "password must be between {MIN_PASSWORD_BYTES} and 1024 bytes"
        )));
    }
    Ok(())
}

pub fn hash_password(password: &str) -> Result<String, StoreError> {
    let salt = SaltString::generate(&mut PasswordRandomSource);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| StoreError::Validation("password hashing failed".to_owned()))
}

pub fn verify_password(password: &str, encoded_hash: &str) -> bool {
    let Ok(hash) = PasswordHash::new(encoded_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &hash)
        .is_ok()
}

pub fn random_token(prefix: &str) -> String {
    let mut bytes = [0_u8; 32];
    TokenRandomSource.fill_bytes(&mut bytes);
    let encoded = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{prefix}-{encoded}")
}

pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn device_token(_device_id: &str) -> String {
    random_token("cairn-device")
}

pub fn redact_sensitive_content(content: &str) -> String {
    let mut redacted = content.to_owned();
    for marker in ["authorization: bearer ", "api_key=", "token=", "password="] {
        redacted = redact_after_marker(&redacted, marker);
    }
    redact_prefixed_secret(&redacted, "sk-")
}

fn redact_after_marker(content: &str, marker: &str) -> String {
    let lower = content.to_ascii_lowercase();
    let mut output = String::with_capacity(content.len());
    let mut cursor = 0;

    while let Some(relative) = lower[cursor..].find(marker) {
        let start = cursor + relative;
        let value_start = start + marker.len();
        output.push_str(&content[cursor..value_start]);
        let value_end = content[value_start..]
            .find(char::is_whitespace)
            .map(|offset| value_start + offset)
            .unwrap_or(content.len());
        output.push_str(REDACTED);
        cursor = value_end;
    }

    output.push_str(&content[cursor..]);
    output
}

fn redact_prefixed_secret(content: &str, prefix: &str) -> String {
    let mut output = String::with_capacity(content.len());
    let mut cursor = 0;
    while let Some(relative) = content[cursor..].find(prefix) {
        let start = cursor + relative;
        let value_end = content[start..]
            .find(char::is_whitespace)
            .map(|offset| start + offset)
            .unwrap_or(content.len());
        output.push_str(&content[cursor..start]);
        output.push_str(REDACTED);
        cursor = value_end;
    }
    output.push_str(&content[cursor..]);
    output
}

struct SessionDigest {
    content: String,
    next_steps: Vec<String>,
    unresolved_work: Vec<String>,
}

fn build_session_digest(observations: &[Observation]) -> SessionDigest {
    let mut content = String::from("Session summary (deterministically compressed):\n");
    let mut seen = HashSet::new();
    let mut omitted = 0;
    let mut next_steps = Vec::new();
    let mut unresolved_work = Vec::new();
    let mut source_actors = Vec::new();
    for observation in observations {
        if !observation.source.trim().is_empty() {
            push_unique(&mut source_actors, observation.source.clone());
        }
        let normalized = observation
            .content
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if !seen.insert(normalized.clone()) {
            omitted += 1;
            continue;
        }
        let line = format!("- {} [evidence: {}]\n", normalized, observation.event_id);
        if content.len() + line.len() > MAX_MEMORY_BYTES {
            omitted += 1;
            continue;
        }
        content.push_str("- ");
        content.push_str(&normalized);
        content.push_str(" [evidence: ");
        content.push_str(&observation.event_id);
        content.push_str("]\n");

        let lower = normalized.to_ascii_lowercase();
        let evidence = format!("{normalized} [evidence: {}]", observation.event_id);
        if contains_marker(
            &lower,
            &[
                "next step",
                "next:",
                "follow up",
                "follow-up",
                "should ",
                "need to ",
                "todo",
                "remaining",
            ],
        ) {
            push_unique(&mut next_steps, evidence.clone());
        }
        if contains_marker(
            &lower,
            &[
                "todo",
                "unresolved",
                "blocked",
                "not yet",
                "not covered",
                "untested",
                "follow up",
                "follow-up",
                "remaining",
            ],
        ) {
            push_unique(&mut unresolved_work, evidence);
        }
    }
    if omitted > 0 {
        content.push_str(&format!("- {omitted} repeated or oversized observations omitted; source IDs remain attached to the memory.\n"));
    }
    append_digest_section(&mut content, "Next steps", &next_steps);
    append_digest_section(&mut content, "Unresolved work", &unresolved_work);
    append_digest_section(&mut content, "Source actors", &source_actors);
    SessionDigest {
        content,
        next_steps,
        unresolved_work,
    }
}

pub fn derive_session_memory(
    session_id: &str,
    session_context: &WorkContext,
    observations: &[Observation],
) -> Result<Memory, StoreError> {
    if session_id.trim().is_empty() {
        return Err(StoreError::Validation(
            "session_id must not be empty".to_owned(),
        ));
    }
    if observations.is_empty() {
        return Err(StoreError::Validation(
            "session has no retained observations".to_owned(),
        ));
    }
    let digest = build_session_digest(observations);
    Ok(Memory {
        memory_id: format!("memory-{session_id}"),
        session_id: session_id.to_owned(),
        content: digest.content,
        source_event_ids: observations
            .iter()
            .map(|observation| observation.event_id.clone())
            .collect(),
        context: merge_context(observations),
        lifecycle: Lifecycle::durable_from(observations),
        kind: MemoryKind::SessionSummary,
        scope: session_scope(session_context),
        project_id: None,
        next_steps: digest.next_steps,
        unresolved_work: digest.unresolved_work,
        supersedes: None,
        promoted_from: None,
        promotion_reason: None,
        verified_commit: None,
        created_at: timestamp(),
    })
}

fn append_digest_section(content: &mut String, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    content.push('\n');
    content.push_str(title);
    content.push_str(":\n");
    for item in items {
        if content.len() + item.len() + 2 > MAX_MEMORY_BYTES {
            break;
        }
        content.push_str("- ");
        content.push_str(item);
        content.push('\n');
    }
}

fn contains_marker(value: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| value.contains(marker))
}

fn push_unique(items: &mut Vec<String>, item: String) {
    if items.len() >= MAX_HANDOFF_ITEMS || items.contains(&item) {
        return;
    }
    if item.len() <= MAX_HANDOFF_ITEM_BYTES {
        items.push(item);
    } else {
        let truncated = item
            .chars()
            .take(MAX_HANDOFF_ITEM_BYTES)
            .collect::<String>();
        items.push(truncated);
    }
}

pub fn recall_result(
    memory: &Memory,
    score: f32,
    all_memories: &[Memory],
    forgotten_ids: &HashSet<String>,
    filter: &RecallFilter,
) -> RecallResult {
    let (stale, stale_reason) = stale_signal(memory, filter);
    let contradiction_ids = contradiction_ids(memory, all_memories, forgotten_ids);
    RecallResult {
        memory_id: memory.memory_id.clone(),
        content: memory.content.clone(),
        source_event_ids: memory.source_event_ids.clone(),
        context: memory.context.clone(),
        lifecycle: memory.lifecycle.clone(),
        scope: memory.scope.clone(),
        project_id: memory.project_id.clone(),
        next_steps: memory.next_steps.clone(),
        unresolved_work: memory.unresolved_work.clone(),
        decision_reason: extract_decision_reason(&memory.content),
        relationships: code_relationships(memory),
        stale,
        stale_reason,
        contradictory: !contradiction_ids.is_empty(),
        contradiction_ids,
        promoted_from: memory.promoted_from.clone(),
        promotion_reason: memory.promotion_reason.clone(),
        verified_commit: memory.verified_commit.clone(),
        score,
    }
}

fn extract_decision_reason(content: &str) -> Option<String> {
    let lower = content.to_ascii_lowercase();
    let start = lower
        .find("because")
        .map(|index| index + "because".len())
        .or_else(|| lower.find("reason:").map(|index| index + "reason:".len()))?;
    let reason = content[start..]
        .split_once("[evidence:")
        .map(|(before, _)| before)
        .unwrap_or(&content[start..])
        .trim()
        .trim_start_matches([':', '-', ' '])
        .trim_end_matches(['.', '!', '?'])
        .trim();
    (!reason.is_empty()).then(|| reason.to_owned())
}

fn code_relationships(memory: &Memory) -> Vec<CodeRelationship> {
    let mut relationships = Vec::new();
    let memory_id = memory.memory_id.as_str();
    for file in &memory.context.files {
        push_relationship(
            &mut relationships,
            "memory_touches_file",
            memory_id,
            file,
            "captured work context",
        );
    }
    for symbol in &memory.context.symbols {
        push_relationship(
            &mut relationships,
            "memory_mentions_symbol",
            memory_id,
            symbol,
            "captured work context",
        );
        for file in &memory.context.files {
            push_relationship(
                &mut relationships,
                "file_contains_symbol",
                file,
                symbol,
                "same observation context",
            );
        }
        for test in &memory.context.tests {
            push_relationship(
                &mut relationships,
                "symbol_verified_by_test",
                symbol,
                test,
                "same observation context",
            );
        }
    }
    for command in &memory.context.commands {
        for test in &memory.context.tests {
            push_relationship(
                &mut relationships,
                "command_verifies_test",
                command,
                test,
                "same observation context",
            );
        }
    }
    for service in &memory.context.services {
        for file in &memory.context.files {
            push_relationship(
                &mut relationships,
                "service_owns_file",
                service,
                file,
                "same observation context",
            );
        }
    }
    relationships
}

fn push_relationship(
    relationships: &mut Vec<CodeRelationship>,
    relation: &str,
    from: &str,
    to: &str,
    evidence: &str,
) {
    if relationships.len() >= MAX_RELATIONSHIPS {
        return;
    }
    relationships.push(CodeRelationship {
        relation: relation.to_owned(),
        from: from.to_owned(),
        to: to.to_owned(),
        evidence: evidence.to_owned(),
    });
}

fn stale_signal(memory: &Memory, filter: &RecallFilter) -> (bool, Option<String>) {
    if memory.lifecycle.freshness < 50 {
        return (true, Some("memory freshness is below 50".to_owned()));
    }
    let Some(current) = filter.context.as_ref() else {
        return (false, None);
    };
    if matches!(memory.scope, MemoryScope::Branch | MemoryScope::Worktree)
        && memory.context.repository == current.repository
        && memory.context.commit.is_some()
        && current.commit.is_some()
        && memory.context.commit != current.commit
    {
        return (
            true,
            Some("memory was captured at a different commit".to_owned()),
        );
    }
    if memory.scope == MemoryScope::Project
        && memory.context.repository == current.repository
        && memory.verified_commit.is_some()
        && current.commit.is_some()
        && memory.verified_commit != current.commit
    {
        return (
            true,
            Some("project memory was verified at a different commit".to_owned()),
        );
    }
    (false, None)
}

pub fn competing_project_memories(memories: &[Memory]) -> Vec<ProjectConflict> {
    let forgotten = HashSet::new();
    let mut seen = HashSet::new();
    let mut conflicts = Vec::new();
    for memory in memories
        .iter()
        .filter(|memory| memory.scope == MemoryScope::Project)
    {
        for other_id in contradiction_ids(memory, memories, &forgotten) {
            let mut pair = [memory.memory_id.clone(), other_id];
            pair.sort();
            if seen.insert((pair[0].clone(), pair[1].clone())) {
                conflicts.push(ProjectConflict {
                    memory_ids: vec![pair[0].clone(), pair[1].clone()],
                });
            }
        }
    }
    conflicts
}

fn candidate_memory(memory: &Memory, project_id: &str) -> bool {
    memory.scope == MemoryScope::Project
        && memory.project_id.as_deref() == Some(project_id)
        && memory.lifecycle.sensitivity != Sensitivity::Secret
}

fn not_auto_shared_reason(policy: &ProjectSharePolicy, memory: &Memory) -> String {
    if *policy == ProjectSharePolicy::Manual {
        "space policy is manual".to_owned()
    } else if memory.lifecycle.sensitivity == Sensitivity::Sensitive {
        "memory is sensitive".to_owned()
    } else {
        "automatic policy did not match this memory".to_owned()
    }
}

fn contradiction_ids(
    memory: &Memory,
    all_memories: &[Memory],
    forgotten_ids: &HashSet<String>,
) -> Vec<String> {
    let target_negative = contains_negation(&memory.content);
    let target_topics = topic_tokens(&memory.content);
    if target_topics.is_empty() {
        return Vec::new();
    }
    all_memories
        .iter()
        .filter(|other| other.memory_id != memory.memory_id)
        .filter(|other| !forgotten_ids.contains(&other.memory_id))
        .filter(|other| same_scope_context(memory, other))
        .filter(|other| contains_negation(&other.content) != target_negative)
        .filter(|other| {
            let other_topics = topic_tokens(&other.content);
            target_topics.intersection(&other_topics).count() >= 2
        })
        .map(|other| other.memory_id.clone())
        .take(8)
        .collect()
}

fn same_scope_context(left: &Memory, right: &Memory) -> bool {
    if left.scope != right.scope {
        return false;
    }
    match left.scope {
        MemoryScope::Global => true,
        MemoryScope::Project => left.context.repository == right.context.repository,
        MemoryScope::Branch => {
            left.context.repository == right.context.repository
                && left.context.branch == right.context.branch
        }
        MemoryScope::Worktree => {
            left.context.repository == right.context.repository
                && left.context.branch == right.context.branch
                && left.context.worktree == right.context.worktree
        }
        MemoryScope::Session => left.session_id == right.session_id,
    }
}

fn contains_negation(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    [
        "do not ",
        "does not ",
        "don't ",
        "never ",
        "avoid ",
        "not use ",
        "not the ",
        "without ",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn topic_tokens(content: &str) -> HashSet<String> {
    let stop_words = [
        "the", "and", "for", "with", "use", "default", "should", "this", "that", "because", "not",
        "do", "does", "is", "are", "as", "to", "a", "an", "of", "in",
    ];
    tokenize(content)
        .into_iter()
        .filter(|term| !stop_words.contains(&term.as_str()))
        .collect()
}

fn build_explicit_memory(content: &str, concepts: Vec<String>) -> String {
    let mut result = redact_sensitive_content(content);
    let concepts: Vec<String> = concepts
        .into_iter()
        .filter(|concept| !concept.trim().is_empty())
        .map(|concept| redact_sensitive_content(&concept))
        .collect();
    if !concepts.is_empty() {
        result.push_str("\nConcepts: ");
        result.push_str(&concepts.join(", "));
    }
    result
}

pub fn build_explicit_memory_record(
    content: &str,
    concepts: Vec<String>,
    context: WorkContext,
    lifecycle: LifecycleInput,
    kind: MemoryKind,
) -> Result<Memory, StoreError> {
    if content.trim().is_empty() {
        return Err(StoreError::Validation(
            "content must not be empty".to_owned(),
        ));
    }
    if content.len() > MAX_OBSERVATION_BYTES {
        return Err(StoreError::Validation(format!(
            "content exceeds the {} byte limit",
            MAX_OBSERVATION_BYTES
        )));
    }
    validate_lifecycle_input(&lifecycle)?;
    let suffix = timestamp_nanos();
    let event_id = format!("event-explicit-{suffix}");
    Ok(Memory {
        memory_id: format!("memory-explicit-{suffix}"),
        session_id: format!("session-explicit-{suffix}"),
        content: build_explicit_memory(content, concepts),
        source_event_ids: vec![event_id],
        context: context.clone(),
        lifecycle: Lifecycle::explicit(&lifecycle, content),
        kind,
        scope: memory_scope(&context),
        project_id: None,
        next_steps: Vec::new(),
        unresolved_work: Vec::new(),
        supersedes: None,
        promoted_from: None,
        promotion_reason: None,
        verified_commit: None,
        created_at: timestamp(),
    })
}

fn merge_context(observations: &[Observation]) -> WorkContext {
    let mut context = WorkContext::default();
    for observation in observations {
        let source = &observation.context;
        context.repository = context
            .repository
            .clone()
            .or_else(|| source.repository.clone());
        context.branch = context.branch.clone().or_else(|| source.branch.clone());
        context.worktree = context.worktree.clone().or_else(|| source.worktree.clone());
        context.commit = context.commit.clone().or_else(|| source.commit.clone());
        for file in &source.files {
            if !context.files.contains(file) {
                context.files.push(file.clone());
            }
        }
        merge_context_items(&mut context.symbols, &source.symbols);
        merge_context_items(&mut context.commands, &source.commands);
        merge_context_items(&mut context.tests, &source.tests);
        merge_context_items(&mut context.services, &source.services);
    }
    context
}

fn merge_context_items(target: &mut Vec<String>, source: &[String]) {
    for item in source {
        if !target.contains(item) {
            target.push(item.clone());
        }
    }
}

fn session_scope(context: &WorkContext) -> MemoryScope {
    if context.repository.is_some() && context.branch.is_some() && context.worktree.is_some() {
        MemoryScope::Worktree
    } else if context.repository.is_some() && context.branch.is_some() {
        MemoryScope::Branch
    } else if context.repository.is_some() {
        MemoryScope::Project
    } else {
        MemoryScope::Session
    }
}

fn memory_scope(context: &WorkContext) -> MemoryScope {
    match session_scope(context) {
        MemoryScope::Session => MemoryScope::Global,
        scope => scope,
    }
}

pub fn memory_matches_filter(memory: &Memory, filter: &RecallFilter) -> bool {
    if filter.session_id.is_none() && filter.context.is_none() {
        return true;
    }

    match memory.scope {
        MemoryScope::Global => true,
        MemoryScope::Project => filter
            .context
            .as_ref()
            .and_then(|context| context.repository.as_ref())
            .is_some_and(|repository| memory.context.repository.as_ref() == Some(repository)),
        MemoryScope::Branch => filter.context.as_ref().is_some_and(|context| {
            context.repository.is_some()
                && context.branch.is_some()
                && memory.context.repository == context.repository
                && memory.context.branch == context.branch
        }),
        MemoryScope::Worktree => filter.context.as_ref().is_some_and(|context| {
            context.repository.is_some()
                && context.branch.is_some()
                && context.worktree.is_some()
                && memory.context.repository == context.repository
                && memory.context.branch == context.branch
                && memory.context.worktree == context.worktree
        }),
        MemoryScope::Session => filter
            .session_id
            .as_ref()
            .is_some_and(|session_id| memory.session_id == *session_id),
    }
}

fn tokenize(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|term| term.len() >= 2)
        .map(|term| term.to_ascii_lowercase())
        .collect()
}

fn score_memory(memory: &Memory, terms: &[String]) -> f32 {
    let haystack = format!(
        "{} {} {} {} {} {} {} {} {}",
        memory.content,
        memory.context.repository.as_deref().unwrap_or_default(),
        memory.context.branch.as_deref().unwrap_or_default(),
        memory.context.worktree.as_deref().unwrap_or_default(),
        memory.context.files.join(" "),
        memory.context.symbols.join(" "),
        memory.context.commands.join(" "),
        memory.context.tests.join(" "),
        memory.context.services.join(" "),
    )
    .to_ascii_lowercase();

    terms
        .iter()
        .map(|term| {
            let count = haystack.matches(term).count();
            count as f32
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn temp_store_path(test_name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "cairn-{test_name}-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn context() -> WorkContext {
        WorkContext {
            repository: Some("cairn-demo".to_owned()),
            branch: Some("feat/auth".to_owned()),
            worktree: Some("wt-auth".to_owned()),
            commit: Some("abc1234".to_owned()),
            files: vec!["src/auth/session.rs".to_owned()],
            symbols: vec!["SessionMiddleware::validate_cookie".to_owned()],
            commands: vec!["cargo test auth_session".to_owned()],
            tests: vec!["missing_signature_is_rejected".to_owned()],
            services: vec!["auth".to_owned()],
            handoffs: Vec::new(),
            runbooks: Vec::new(),
        }
    }

    fn observation(event_id: &str, content: &str) -> ObservationInput {
        observation_for("sess-auth-001", event_id, content)
    }

    fn observation_for(session_id: &str, event_id: &str, content: &str) -> ObservationInput {
        ObservationInput {
            event_id: event_id.to_owned(),
            session_id: session_id.to_owned(),
            content: content.to_owned(),
            role: "assistant".to_owned(),
            source: "test".to_owned(),
            importance: Some("high".to_owned()),
            lifecycle: LifecycleInput::default(),
            context: context(),
        }
    }

    #[test]
    fn captures_and_recalls_evidence_backed_session_memory() -> Result<(), StoreError> {
        let path = temp_store_path("continuity");
        let mut store = MemoryStore::open(&path)?;
        store.start_session("sess-auth-001", context())?;
        store.capture_observation(observation(
            "evt-session-002",
            "The session middleware validates the signed cookie before loading the user.",
        ))?;
        store.capture_observation(observation(
            "evt-session-003",
            "The missing signature test passed.",
        ))?;

        let memory = store.end_session("sess-auth-001")?;
        assert!(memory.content.contains("missing signature"));
        assert_eq!(memory.source_event_ids.len(), 2);
        assert_eq!(memory.scope, MemoryScope::Worktree);
        assert_eq!(memory.context.symbols, context().symbols);
        assert_eq!(memory.context.commands, context().commands);
        assert_eq!(memory.context.tests, context().tests);
        assert_eq!(memory.context.services, context().services);

        let results = store.search("missing signature", 5)?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory_id, "memory-sess-auth-001");
        assert!(
            results[0]
                .source_event_ids
                .contains(&"evt-session-003".to_owned())
        );
        assert_eq!(results[0].scope, MemoryScope::Worktree);
        assert!(
            !store
                .search("SessionMiddleware validate_cookie", 5)?
                .is_empty()
        );

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn session_handoff_extracts_next_steps_and_unresolved_work() -> Result<(), StoreError> {
        let path = temp_store_path("handoff");
        let mut store = MemoryStore::open(&path)?;
        store.start_session("sess-handoff", context())?;
        store.capture_observation(observation_for(
            "sess-handoff",
            "evt-handoff",
            "The next step is to add the missing signature regression test; the issue is not yet verified.",
        ))?;

        let memory = store.end_session("sess-handoff")?;
        assert_eq!(memory.next_steps.len(), 1);
        assert_eq!(memory.unresolved_work.len(), 1);
        assert!(memory.content.contains("Next steps:"));
        assert!(memory.content.contains("Unresolved work:"));

        let filter = RecallFilter {
            session_id: None,
            context: Some(context()),
        };
        let resume = store.resume_context(5, &filter)?;
        assert_eq!(resume.results.len(), 1);
        assert_eq!(resume.results[0].next_steps, memory.next_steps);

        let unresolved = store.unresolved_context(5, &filter)?;
        assert_eq!(unresolved.results.len(), 1);
        assert_eq!(
            unresolved.results[0].unresolved_work,
            memory.unresolved_work
        );

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn why_changed_reports_reason_relationships_staleness_and_contradictions()
    -> Result<(), StoreError> {
        let path = temp_store_path("why-changed");
        let mut store = MemoryStore::open(&path)?;
        let project_context = WorkContext {
            repository: Some("cairn-demo".to_owned()),
            files: vec!["src/cache.rs".to_owned()],
            symbols: vec!["Cache::get".to_owned()],
            commands: vec!["cargo test cache".to_owned()],
            tests: vec!["cache_reads_are_consistent".to_owned()],
            services: vec!["cache".to_owned()],
            ..WorkContext::default()
        };
        let stale_context = WorkContext {
            repository: Some("cairn-demo".to_owned()),
            branch: Some("feat/cache".to_owned()),
            worktree: Some("wt-cache".to_owned()),
            commit: Some("abc1234".to_owned()),
            ..WorkContext::default()
        };
        let current_context = WorkContext {
            commit: Some("def5678".to_owned()),
            ..stale_context.clone()
        };
        let first = store.remember(
            "The project uses Redis as the default cache because reads are fast.",
            Vec::new(),
            project_context,
        )?;
        let second = store.remember(
            "The project does not use Redis as the default cache because consistency is preferred.",
            Vec::new(),
            WorkContext {
                repository: Some("cairn-demo".to_owned()),
                ..WorkContext::default()
            },
        )?;
        let stale = store.remember_with_lifecycle(
            "The cache experiment remains valid because the old benchmark passed.",
            Vec::new(),
            stale_context,
            LifecycleInput {
                freshness: Some(40),
                ..LifecycleInput::default()
            },
        )?;

        let filter = RecallFilter {
            session_id: None,
            context: Some(current_context),
        };
        let pack = store.why_changed_context("Redis default cache", 10, &filter)?;
        let first_result = pack
            .results
            .iter()
            .find(|result| result.memory_id == first.memory_id)
            .unwrap();
        let second_result = pack
            .results
            .iter()
            .find(|result| result.memory_id == second.memory_id)
            .unwrap();
        let stale_result = pack
            .results
            .iter()
            .find(|result| result.memory_id == stale.memory_id)
            .unwrap();
        assert_eq!(
            first_result.decision_reason.as_deref(),
            Some("reads are fast")
        );
        assert!(first_result.relationships.iter().any(|relationship| {
            relationship.relation == "file_contains_symbol"
                && relationship.from == "src/cache.rs"
                && relationship.to == "Cache::get"
        }));
        assert!(first_result.relationships.iter().any(|relationship| {
            relationship.relation == "symbol_verified_by_test"
                && relationship.to == "cache_reads_are_consistent"
        }));
        assert!(second_result.contradictory);
        assert!(second_result.contradiction_ids.contains(&first.memory_id));
        assert!(stale_result.stale);
        assert_eq!(
            stale_result.stale_reason.as_deref(),
            Some("memory freshness is below 50")
        );

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn trusted_device_sync_is_authenticated_idempotent_and_revocable() -> Result<(), StoreError> {
        let path = temp_store_path("device-sync");
        let mut store = MemoryStore::open(&path)?;
        let first = store.register_device(
            "device-a",
            "person-1",
            Some("agent-a".to_owned()),
            "MacBook",
        )?;
        let second = store.register_device("device-b", "person-1", None, "Laptop")?;
        assert_eq!(first.device.status, DeviceStatus::Active);
        assert_eq!(second.device.status, DeviceStatus::Active);

        let input = observation_for(
            "sync-session",
            "sync-event-1",
            "The trusted device sync test passed.",
        );
        let pushed = store.sync_push(&first.token, input.clone())?;
        assert!(pushed.accepted);
        assert!(!pushed.duplicate);
        let duplicate = store.sync_push(&first.token, input)?;
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.cursor, pushed.cursor);

        let pulled = store.sync_pull(&second.token, 0)?;
        assert_eq!(pulled.events.len(), 1);
        assert_eq!(pulled.events[0].event_id, "sync-event-1");
        let resumed = store.sync_pull(&second.token, pulled.cursor)?;
        assert!(resumed.events.is_empty());

        drop(store);
        let mut restored = MemoryStore::open(&path)?;
        let restored_pull = restored.sync_pull(&second.token, 0)?;
        assert_eq!(restored_pull.events.len(), 1);
        assert_eq!(restored_pull.events[0].event_id, "sync-event-1");

        let rotated = restored.rotate_device_token(&first.token, "device-a")?;
        assert_ne!(rotated.token, first.token);
        assert!(
            restored
                .sync_push(
                    &first.token,
                    observation_for("sync-session", "sync-event-old-token", "old token"),
                )
                .is_err()
        );

        let revoked = restored.revoke_device(&rotated.token, "device-b")?;
        assert_eq!(revoked.status, DeviceStatus::Revoked);
        assert!(restored.sync_pull(&second.token, pulled.cursor).is_err());
        assert!(restored.authenticate_device(&rotated.token).is_ok());
        assert!(restored.authenticate_device(&second.token).is_err());

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn account_auth_hashes_credentials_and_persists_revocable_sessions() -> Result<(), StoreError> {
        let path = temp_store_path("account-auth");
        let password = "correct horse battery staple";
        let mut store = MemoryStore::open(&path)?;
        let bootstrap = store.bootstrap_account("Owner@Example.test", password)?;
        assert_eq!(bootstrap.account.email, "owner@example.test");
        assert!(store.authenticate_account(&bootstrap.token).is_ok());
        assert!(!fs::read_to_string(&path)?.contains(password));
        assert!(!fs::read_to_string(&path)?.contains(&bootstrap.token));

        let login = store.login("owner@example.test", password)?;
        assert_ne!(login.token, bootstrap.token);
        assert!(store.login("owner@example.test", "wrong password").is_err());
        assert!(
            store
                .bootstrap_account("second@example.test", password)
                .is_err()
        );

        drop(store);
        let mut restored = MemoryStore::open(&path)?;
        assert!(restored.authenticate_account(&login.token).is_ok());
        restored.logout_account(&login.token)?;
        assert!(restored.authenticate_account(&login.token).is_err());

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn sync_event_id_collision_cannot_leak_another_persons_observation() -> Result<(), StoreError> {
        let path = temp_store_path("device-sync-scope");
        let mut store = MemoryStore::open(&path)?;
        let owner = store.register_device("device-owner", "person-owner", None, "Owner")?;
        let other = store.register_device("device-other", "person-other", None, "Other")?;

        store.sync_push(
            &owner.token,
            observation_for("owner-session", "shared-event", "owner private evidence"),
        )?;

        let error = store
            .sync_push(
                &other.token,
                observation_for("other-session", "shared-event", "other retry"),
            )
            .expect_err("a cross-person event ID collision must not be acknowledged");
        assert!(error.to_string().contains("another person"));

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn invalid_observation_and_lifecycle_input_is_rejected_before_persistence()
    -> Result<(), StoreError> {
        let path = temp_store_path("validation-boundaries");
        let mut store = MemoryStore::open(&path)?;

        let mut invalid = observation("invalid-empty-role", "should not persist");
        invalid.role.clear();
        assert!(store.capture_observation(invalid).is_err());

        let mut invalid = observation("invalid-importance", "should not persist");
        invalid.lifecycle.importance = Some(101);
        assert!(store.capture_observation(invalid).is_err());

        let mut invalid = observation("invalid-expiry", "should not persist");
        invalid.lifecycle.expires_at = Some("not-a-timestamp".to_owned());
        assert!(store.capture_observation(invalid).is_err());

        assert_eq!(store.observations().count(), 0);
        assert!(!path.exists());

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn conflicting_import_is_atomic() -> Result<(), StoreError> {
        let source_path = temp_store_path("import-conflict-source");
        let target_path = temp_store_path("import-conflict-target");
        let mut source = MemoryStore::open(&source_path)?;
        let imported =
            source.remember("new imported memory", Vec::new(), WorkContext::default())?;
        let mut bundle = source.export();

        let mut target = MemoryStore::open(&target_path)?;
        let existing =
            target.remember("existing target memory", Vec::new(), WorkContext::default())?;
        let mut conflicting = existing.clone();
        conflicting.content = "conflicting replacement".to_owned();
        bundle.memories = vec![imported.clone(), conflicting];

        let error = target
            .import_bundle(bundle)
            .expect_err("conflicting imports must fail");
        assert!(error.to_string().contains("conflicting memory"));
        assert!(
            !target
                .search("new imported memory", 5)?
                .iter()
                .any(|result| result.memory_id == imported.memory_id)
        );
        assert_eq!(target.search("existing target memory", 5)?.len(), 1);

        fs::remove_file(source_path)?;
        fs::remove_file(target_path)?;
        Ok(())
    }

    #[test]
    fn scoped_recall_keeps_branch_memory_isolated() -> Result<(), StoreError> {
        let path = temp_store_path("scoped-recall");
        let mut store = MemoryStore::open(&path)?;
        let project_context = WorkContext {
            repository: Some("cairn-demo".to_owned()),
            ..WorkContext::default()
        };
        let main_context = WorkContext {
            repository: Some("cairn-demo".to_owned()),
            branch: Some("main".to_owned()),
            worktree: Some("wt-main".to_owned()),
            commit: Some("main222".to_owned()),
            files: Vec::new(),
            ..WorkContext::default()
        };

        let project = store.remember(
            "The project cache convention is documented and verified.",
            Vec::new(),
            project_context,
        )?;
        let branch = store.remember(
            "The Redis cache experiment is a branch-local hypothesis.",
            Vec::new(),
            context(),
        )?;
        let global = store.remember(
            "The user prefers evidence-backed cache decisions.",
            Vec::new(),
            WorkContext::default(),
        )?;

        assert_eq!(project.scope, MemoryScope::Project);
        assert_eq!(branch.scope, MemoryScope::Worktree);
        assert_eq!(global.scope, MemoryScope::Global);

        let main_filter = RecallFilter {
            session_id: None,
            context: Some(main_context.clone()),
        };
        let main_results = store.search_with_filter("cache", 10, &main_filter)?;
        let main_ids: HashSet<_> = main_results
            .iter()
            .map(|result| result.memory_id.as_str())
            .collect();
        assert!(main_ids.contains(project.memory_id.as_str()));
        assert!(main_ids.contains(global.memory_id.as_str()));
        assert!(!main_ids.contains(branch.memory_id.as_str()));

        let branch_filter = RecallFilter {
            session_id: None,
            context: Some(context()),
        };
        let branch_results = store.search_with_filter("cache", 10, &branch_filter)?;
        let branch_ids: HashSet<_> = branch_results
            .iter()
            .map(|result| result.memory_id.as_str())
            .collect();
        assert!(branch_ids.contains(project.memory_id.as_str()));
        assert!(branch_ids.contains(global.memory_id.as_str()));
        assert!(branch_ids.contains(branch.memory_id.as_str()));

        let pack = store.context_with_filter("cache", 2, &branch_filter)?;
        assert_eq!(pack.results.len(), 2);
        assert_eq!(pack.omitted_count, 1);
        assert_eq!(pack.omitted_memory_ids, vec![global.memory_id.clone()]);
        assert!(pack.recovery_hint.is_some());

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn redacts_sensitive_values_before_persistence() -> Result<(), StoreError> {
        let path = temp_store_path("redaction");
        let mut store = MemoryStore::open(&path)?;
        store.start_session("sess-secret-001", context())?;
        let result = store.capture_observation(observation(
            "evt-sensitive-001",
            "Authorization: Bearer real-test-token api_key=another-test-key",
        ))?;

        assert!(!result.observation.content.contains("real-test-token"));
        assert!(!result.observation.content.contains("another-test-key"));
        assert_eq!(result.observation.content.matches(REDACTED).count(), 2);

        let persisted = fs::read_to_string(&path)?;
        assert!(!persisted.contains("real-test-token"));
        assert!(!persisted.contains("another-test-key"));

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn duplicate_event_is_acknowledged_without_duplication() -> Result<(), StoreError> {
        let path = temp_store_path("deduplication");
        let mut store = MemoryStore::open(&path)?;
        store.start_session("sess-auth-001", context())?;
        let first = store.capture_observation(observation("evt-duplicate", "same event"))?;
        let second = store.capture_observation(observation("evt-duplicate", "different retry"))?;

        assert!(!first.duplicate);
        assert!(second.duplicate);
        assert_eq!(store.observations().count(), 1);
        assert_eq!(second.observation.content, "same event");

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn capture_inherits_context_from_started_session() -> Result<(), StoreError> {
        let path = temp_store_path("session-context");
        let mut store = MemoryStore::open(&path)?;
        store.start_session("sess-context", context())?;
        let captured = store.capture_observation(ObservationInput {
            event_id: "evt-context".to_owned(),
            session_id: "sess-context".to_owned(),
            content: "The session context should follow this capture.".to_owned(),
            role: "agent".to_owned(),
            source: "cli".to_owned(),
            importance: None,
            lifecycle: LifecycleInput::default(),
            context: WorkContext::default(),
        })?;

        assert_eq!(captured.observation.context, context());
        let memory = store.end_session("sess-context")?;
        assert_eq!(memory.scope, MemoryScope::Worktree);

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn forget_suppresses_derived_memory_after_restart() -> Result<(), StoreError> {
        let path = temp_store_path("forget");
        {
            let mut store = MemoryStore::open(&path)?;
            store.start_session("sess-auth-001", context())?;
            store.capture_observation(observation("evt-forget", "private debugging note"))?;
            store.end_session("sess-auth-001")?;
            store.forget("evt-forget")?;
            assert!(store.search("private debugging note", 5)?.is_empty());
        }

        let store = MemoryStore::open(&path)?;
        assert!(store.search("private debugging note", 5)?.is_empty());

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn reopening_store_preserves_session_memory() -> Result<(), StoreError> {
        let path = temp_store_path("restart");
        {
            let mut store = MemoryStore::open(&path)?;
            store.start_session("sess-auth-001", context())?;
            store.capture_observation(observation(
                "evt-restart",
                "restart persistence keeps the session evidence",
            ))?;
            store.end_session("sess-auth-001")?;
        }

        let store = MemoryStore::open(&path)?;
        let results = store.search("restart persistence", 5)?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source_event_ids, vec!["evt-restart"]);

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn session_continuity_fixture_runs_through_capture_to_recall() -> Result<(), StoreError> {
        let path = temp_store_path("session-fixture");
        let mut store = MemoryStore::open(&path)?;
        let mut memory = None;

        for line in include_str!("../../../fixtures/phase-0/session-continuity.jsonl").lines() {
            let record: Value = serde_json::from_str(line).expect("fixture record must be JSON");
            let event_type = record["type"].as_str().expect("fixture type");
            let session_id = record["sessionId"].as_str().expect("fixture session");
            let context = context_from_fixture(&record["context"]);

            match event_type {
                "session.start" => {
                    store.start_session(session_id, context)?;
                }
                "observation" | "test" => {
                    let payload = &record["payload"];
                    let content = payload["content"]
                        .as_str()
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| {
                            format!(
                                "{} {} {:?}",
                                payload["command"].as_str().unwrap_or_default(),
                                payload["result"].as_str().unwrap_or_default(),
                                payload["tests"]
                            )
                        });
                    store.capture_observation(ObservationInput {
                        event_id: record["eventId"].as_str().unwrap().to_owned(),
                        session_id: session_id.to_owned(),
                        content,
                        role: payload["role"].as_str().unwrap_or("tool").to_owned(),
                        source: payload["source"].as_str().unwrap_or(event_type).to_owned(),
                        importance: None,
                        lifecycle: LifecycleInput::default(),
                        context,
                    })?;
                }
                "session.end" => {
                    memory = Some(store.end_session(session_id)?);
                }
                other => panic!("unexpected fixture event type: {other}"),
            }
        }

        let memory = memory.expect("fixture must end the session");
        assert_eq!(memory.source_event_ids.len(), 2);
        let pack = store.context("missing signature", 5)?;
        assert_eq!(pack.results.len(), 1);
        assert!(
            pack.results[0]
                .source_event_ids
                .contains(&"evt-session-003".to_owned())
        );

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn rejects_unbounded_observations() {
        let input = observation("evt-too-large", &"x".repeat(MAX_OBSERVATION_BYTES + 1));
        let path = temp_store_path("bounded-input");
        let mut store = MemoryStore::open(&path).expect("store opens");
        let error = store
            .capture_observation(input)
            .expect_err("oversized observation must be rejected");
        assert!(error.to_string().contains("byte limit"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn export_import_round_trip_is_idempotent_and_preserves_forget() -> Result<(), StoreError> {
        let source_path = temp_store_path("export-source");
        let target_path = temp_store_path("export-target");
        let bundle = {
            let mut source = MemoryStore::open(&source_path)?;
            source.start_session("sess-export", context())?;
            source.capture_observation(ObservationInput {
                event_id: "evt-export".to_owned(),
                session_id: "sess-export".to_owned(),
                content: "Exported evidence must survive a restore without replaying providers."
                    .to_owned(),
                role: "assistant".to_owned(),
                source: "test".to_owned(),
                importance: Some("high".to_owned()),
                lifecycle: LifecycleInput::default(),
                context: context(),
            })?;
            source.end_session("sess-export")?;
            source.forget("evt-export")?;
            source.export()
        };

        assert_eq!(bundle.format, EXPORT_FORMAT);
        assert_eq!(bundle.version, EXPORT_VERSION);
        assert!(bundle.forgotten_ids.contains(&"evt-export".to_owned()));

        let mut target = MemoryStore::open(&target_path)?;
        let first = target.import_bundle(bundle.clone())?;
        assert_eq!(first.imported_sessions, 1);
        assert_eq!(first.imported_observations, 1);
        assert_eq!(first.imported_memories, 1);
        assert_eq!(first.imported_forgotten_ids, 1);
        assert!(target.search("providers", 5)?.is_empty());

        let second = target.import_bundle(bundle)?;
        assert_eq!(
            second,
            ImportResult {
                imported_sessions: 0,
                imported_observations: 0,
                imported_memories: 0,
                imported_forgotten_ids: 0,
            }
        );
        drop(target);
        let restored = MemoryStore::open(&target_path)?;
        assert!(restored.search("providers", 5)?.is_empty());

        fs::remove_file(source_path)?;
        fs::remove_file(target_path)?;
        Ok(())
    }

    #[test]
    fn import_rejects_unknown_version_and_redacts_payload() -> Result<(), StoreError> {
        let path = temp_store_path("export-validation");
        let mut store = MemoryStore::open(&path)?;
        let invalid = serde_json::json!({
            "format": EXPORT_FORMAT,
            "version": 99,
            "exportedAt": "0",
            "sessions": [],
            "observations": [],
            "memories": [],
            "forgottenIds": []
        });
        let error = store
            .import_json(&invalid.to_string())
            .expect_err("version must be rejected");
        assert!(error.to_string().contains("unsupported export version"));

        let bundle = ExportBundle {
            format: EXPORT_FORMAT.to_owned(),
            version: EXPORT_VERSION,
            exported_at: "0".to_owned(),
            sessions: vec![],
            observations: vec![],
            memories: vec![Memory {
                memory_id: "memory-import-secret".to_owned(),
                session_id: "session-import-secret".to_owned(),
                content: "api_key=should-not-persist".to_owned(),
                source_event_ids: vec![],
                context: WorkContext::default(),
                lifecycle: Lifecycle::default(),
                kind: MemoryKind::Explicit,
                scope: MemoryScope::Global,
                project_id: None,
                next_steps: Vec::new(),
                unresolved_work: Vec::new(),
                supersedes: None,
                promoted_from: None,
                promotion_reason: None,
                verified_commit: None,
                created_at: "0".to_owned(),
            }],
            forgotten_ids: vec![],
        };
        store.import_bundle(bundle)?;
        let persisted = fs::read_to_string(&path)?;
        assert!(!persisted.contains("should-not-persist"));
        assert!(persisted.contains(REDACTED));
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn classifies_lifecycle_dimensions_and_excludes_expired_memory() -> Result<(), StoreError> {
        let path = temp_store_path("lifecycle");
        let mut store = MemoryStore::open(&path)?;
        store.start_session("sess-lifecycle", context())?;
        let capture = store.capture_observation(ObservationInput {
            event_id: "evt-lifecycle".to_owned(),
            session_id: "sess-lifecycle".to_owned(),
            content: "Private release decision with password=redacted-value".to_owned(),
            role: "agent".to_owned(),
            source: "test".to_owned(),
            importance: Some("high".to_owned()),
            lifecycle: LifecycleInput {
                confidence: Some(90),
                freshness: Some(70),
                ..LifecycleInput::default()
            },
            context: context(),
        })?;
        assert_eq!(capture.observation.lifecycle.importance, 80);
        assert_eq!(
            capture.observation.lifecycle.sensitivity,
            Sensitivity::Secret
        );
        assert_eq!(capture.observation.lifecycle.confidence, 90);
        assert_eq!(capture.observation.lifecycle.freshness, 70);
        assert_eq!(
            capture.observation.lifecycle.retention,
            Retention::Temporary
        );

        let memory = store.remember_with_lifecycle(
            "A temporary note that should expire.",
            vec![],
            WorkContext::default(),
            LifecycleInput {
                retention: Some(Retention::Temporary),
                expires_at: Some((now_seconds() - 1).to_string()),
                ..LifecycleInput::default()
            },
        )?;
        assert!(store.search("temporary note", 5)?.is_empty());
        let summary = store.lifecycle_summary();
        assert_eq!(summary.temporary_observations, 1);
        assert_eq!(summary.durable_memories, 0);
        assert!(summary.sensitive_items >= 1);
        assert_eq!(summary.expired_items, 1);
        assert_eq!(memory.lifecycle.retention, Retention::Temporary);

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn older_store_records_receive_default_lifecycle_values() -> Result<(), StoreError> {
        let path = temp_store_path("legacy-lifecycle");
        let legacy = serde_json::json!({
            "version": STORE_VERSION,
            "sessions": [],
            "observations": [],
            "memories": [{
                "memory_id": "memory-legacy",
                "session_id": "session-legacy",
                "content": "legacy memory",
                "source_event_ids": [],
                "context": {},
                "created_at": "0"
            }],
            "forgotten_ids": []
        });
        fs::write(&path, serde_json::to_string(&legacy)?)?;
        let store = MemoryStore::open(&path)?;
        let results = store.search("legacy", 5)?;
        assert_eq!(results[0].lifecycle, Lifecycle::default());
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn lessons_and_corrections_preserve_kind_and_supersession() -> Result<(), StoreError> {
        let path = temp_store_path("correction");
        let mut store = MemoryStore::open(&path)?;
        let lesson = store.remember_lesson(
            "Do not replay provider calls during import.",
            vec!["portability".to_owned()],
            context(),
            LifecycleInput::default(),
        )?;
        assert_eq!(lesson.kind, MemoryKind::Lesson);

        let correction = store.correct(
            &lesson.memory_id,
            "Do not replay provider calls during import; preserve the local tombstone.",
            Vec::new(),
            WorkContext::default(),
            LifecycleInput {
                confidence: Some(40),
                ..LifecycleInput::default()
            },
        )?;
        assert_eq!(correction.kind, MemoryKind::Correction);
        assert_eq!(correction.lifecycle.confidence, 100);
        assert_eq!(correction.supersedes, Some(lesson.memory_id.clone()));
        let corrected = store.search("local tombstone", 5)?;
        assert_eq!(corrected.len(), 1);
        assert_eq!(corrected[0].memory_id, correction.memory_id);
        let shared = store.search("replay provider calls", 5)?;
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].memory_id, correction.memory_id);
        let exported = store.export();
        assert!(
            exported
                .memories
                .iter()
                .any(|memory| memory.memory_id == lesson.memory_id)
        );
        assert!(
            !exported
                .forgotten_ids
                .iter()
                .any(|id| id == &lesson.memory_id)
        );

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn correction_of_a_project_memory_keeps_project_id_and_hides_the_old_listing()
    -> Result<(), StoreError> {
        let path = temp_store_path("correction-project");
        let mut store = MemoryStore::open(&path)?;
        let account =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let project =
            store.register_project(&account.token, "cairn", "Cairn", Some("main".to_owned()))?;
        let source = store.remember(
            "The cache decision passed.",
            Vec::new(),
            WorkContext {
                repository: Some("cairn".to_owned()),
                ..WorkContext::default()
            },
        )?;
        let promoted = store.promote_memory(
            &account.token,
            &project.project_id,
            &source.memory_id,
            "Confirmed by the cache regression test",
        )?;
        let correction = store.correct(
            &promoted.memory_id,
            "The cache decision passed after review.",
            Vec::new(),
            WorkContext::default(),
            LifecycleInput {
                confidence: Some(10),
                ..LifecycleInput::default()
            },
        )?;
        assert_eq!(
            correction.project_id.as_deref(),
            Some(project.project_id.as_str())
        );
        assert_eq!(correction.lifecycle.confidence, 100);
        let listed = store.project_memories(&account.token, &project.project_id)?;
        assert!(
            listed
                .iter()
                .any(|memory| memory.memory_id == correction.memory_id)
        );
        assert!(
            listed
                .iter()
                .all(|memory| memory.memory_id != promoted.memory_id)
        );
        assert!(
            store
                .export()
                .memories
                .iter()
                .any(|memory| memory.memory_id == promoted.memory_id)
        );
        let report = store.project_maintenance(&account.token, &project.project_id, None)?;
        assert!(
            report
                .superseded_memory_ids
                .iter()
                .any(|id| id == &promoted.memory_id)
        );
        assert!(
            report
                .active_memory_ids
                .iter()
                .any(|id| id == &correction.memory_id)
        );
        assert!(
            report
                .active_memory_ids
                .iter()
                .all(|id| id != &promoted.memory_id)
        );

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn opening_a_store_sweeps_expired_records_into_tombstones() -> Result<(), StoreError> {
        let path = temp_store_path("sweep");
        let mut store = MemoryStore::open(&path)?;
        store.remember_with_lifecycle(
            "This memory is already expired.",
            Vec::new(),
            WorkContext::default(),
            LifecycleInput {
                expires_at: Some((now_seconds() - 1).to_string()),
                retention: Some(Retention::Temporary),
                ..LifecycleInput::default()
            },
        )?;
        drop(store);

        let reopened = MemoryStore::open(&path)?;
        assert!(reopened.search("already expired", 5)?.is_empty());
        assert!(
            reopened
                .export()
                .forgotten_ids
                .iter()
                .any(|id| id.starts_with("memory-explicit-"))
        );
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn session_summary_deduplicates_repeated_observations() -> Result<(), StoreError> {
        let path = temp_store_path("compression");
        let mut store = MemoryStore::open(&path)?;
        store.start_session("sess-compress", context())?;
        for event_id in ["evt-compress-a", "evt-compress-b"] {
            store.capture_observation(ObservationInput {
                event_id: event_id.to_owned(),
                session_id: "sess-compress".to_owned(),
                content: "The same verified result was reported twice.".to_owned(),
                role: "tool".to_owned(),
                source: "test".to_owned(),
                importance: Some("medium".to_owned()),
                lifecycle: LifecycleInput::default(),
                context: context(),
            })?;
        }
        let summary = store.end_session("sess-compress")?;
        assert!(summary.content.contains("deterministically compressed"));
        assert!(summary.content.contains("1 repeated"));
        assert!(summary.content.contains("Source actors"));
        assert!(summary.content.contains("test"));
        assert_eq!(summary.source_event_ids.len(), 2);
        fs::remove_file(path)?;
        Ok(())
    }

    fn context_from_fixture(value: &Value) -> WorkContext {
        WorkContext {
            repository: value["repository"].as_str().map(ToOwned::to_owned),
            branch: value["branch"].as_str().map(ToOwned::to_owned),
            worktree: value["worktree"].as_str().map(ToOwned::to_owned),
            commit: value["commit"].as_str().map(ToOwned::to_owned),
            files: value["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect(),
            symbols: context_items(value, "symbols"),
            commands: context_items(value, "commands"),
            tests: context_items(value, "tests"),
            services: context_items(value, "services"),
            handoffs: context_items(value, "handoffs"),
            runbooks: context_items(value, "runbooks"),
        }
    }

    fn context_items(value: &Value, key: &str) -> Vec<String> {
        value[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
            .collect()
    }
}
