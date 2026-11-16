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
use std::collections::{HashMap, HashSet};
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
/// Servers map this error text to 403: a device token reached a web-only action.
pub const WEB_ONLY_ERROR: &str = "web console session required; device tokens cannot do this";
/// Servers map this error text to 403: a non-admin web session reached an admin action.
pub const ADMIN_ONLY_ERROR: &str = "admin web session required; only the admin account can do this";
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
    fn observation(
        input: &LifecycleInput,
        importance_hint: Option<&str>,
        content: &str,
        context: &WorkContext,
    ) -> Self {
        let retention = input.retention.clone().unwrap_or_default();
        Self {
            importance: input
                .importance
                .or_else(|| importance_hint.map(importance_score))
                .unwrap_or(50),
            sensitivity: floor_sensitivity(input.sensitivity.as_ref(), content, &[], context),
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

    fn explicit(
        input: &LifecycleInput,
        content: &str,
        concepts: &[String],
        context: &WorkContext,
    ) -> Self {
        let retention = input.retention.clone().unwrap_or(Retention::Durable);
        Self {
            importance: input.importance.unwrap_or(80),
            sensitivity: floor_sensitivity(input.sensitivity.as_ref(), content, concepts, context),
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
    /// An event id that is already taken by different content or by a row outside sync.
    Conflict(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "storage error: {error}"),
            Self::Json(error) => write!(f, "store format error: {error}"),
            Self::Validation(message) => write!(f, "validation error: {message}"),
            Self::Conflict(message) => write!(f, "conflict: {message}"),
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
    /// The committed `.cairn/project.json` project id. A capture that names an existing
    /// project the person does not own makes the person a member of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
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
    /// With `session_id`, the file session key. Absent on a legacy or unscoped row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_id: Option<String>,
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
    /// Only the bootstrap account is an admin; it alone may create accounts.
    #[serde(default)]
    pub is_admin: bool,
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
    observation_owners: Vec<ObservationOwnerRecord>,
    #[serde(default)]
    sync_deletions: Vec<SyncDeletionRecord>,
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
            observation_owners: Vec::new(),
            sync_deletions: Vec::new(),
            project_members: Vec::new(),
            project_shares: Vec::new(),
            project_policies: Vec::new(),
            dismissed_share_candidates: Vec::new(),
            project_teams: Vec::new(),
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
pub struct SyncDeletion {
    pub cursor: u64,
    pub item_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncPullResult {
    pub cursor: u64,
    pub events: Vec<SyncEvent>,
    /// Deletion instructions for items this device may already have stored.
    /// A device that never pulls again is left unchanged.
    #[serde(default)]
    pub deletions: Vec<SyncDeletion>,
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
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMemoryShare {
    pub project_id: String,
    pub memory_id: String,
    pub member_person_id: String,
    /// Set only on revocation reports, where the owner needs a readable member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_email: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
}

/// One person of a project: its owner, or a member who joined by capture.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectPerson {
    pub person_id: String,
    pub email: String,
    /// `owner` or `member`.
    pub role: String,
    pub joined_at: String,
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
struct ObservationOwnerRecord {
    event_id: String,
    person_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SyncDeletionRecord {
    cursor: u64,
    person_id: String,
    item_id: String,
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
    token_hash: String,
}

/// An open session whose newest observation is older than this is finished on the next capture
/// or session start of the same person.
const IDLE_SESSION_SECS: u64 = 30 * 60;
/// At most this many idle sessions are finished per person per trigger; the next one continues.
const MAX_SWEEP_SESSIONS: usize = 16;

pub struct MemoryStore {
    path: PathBuf,
    state: StoreState,
    /// Last state known to be on disk. A failed `persist` restores it.
    persisted: StoreState,
}

fn share_view(record: &ProjectShareRecord) -> ProjectMemoryShare {
    ProjectMemoryShare {
        project_id: record.project_id.clone(),
        memory_id: record.memory_id.clone(),
        member_person_id: record.member_person_id.clone(),
        member_email: None,
        created_at: record.created_at.clone(),
        revoked_at: record.revoked_at.clone(),
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

/// Adds a tombstone for each expired row that is not a protected source. Pure staging: the
/// caller persists.
fn sweep_expired_state(state: &mut StoreState) -> usize {
    let protected_sources = state
        .memories
        .iter()
        .filter(|memory| keeps_expired_source(memory))
        .filter(|memory| !state.forgotten_ids.contains(&memory.memory_id))
        .flat_map(|memory| memory.source_event_ids.clone())
        .collect::<HashSet<_>>();
    let mut expired_ids = Vec::new();
    for observation in &state.observations {
        if is_expired(&observation.lifecycle) && !protected_sources.contains(&observation.event_id)
        {
            expired_ids.push(observation.event_id.clone());
        }
    }
    for memory in &state.memories {
        if is_expired(&memory.lifecycle) {
            expired_ids.push(memory.memory_id.clone());
        }
    }
    expired_ids
        .into_iter()
        .filter(|id| state.forgotten_ids.insert(id.clone()))
        .count()
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

        let mut store = Self {
            path,
            persisted: state.clone(),
            state,
        };
        store.sweep_expired()?;
        Ok(store)
    }

    pub fn sweep_expired(&mut self) -> Result<usize, StoreError> {
        let added = sweep_expired_state(&mut self.state);
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
        self.start_scoped_session(None, session_id, context)
    }

    pub fn start_session_for_account(
        &mut self,
        account_token: &str,
        session_id: impl Into<String>,
        context: WorkContext,
    ) -> Result<Session, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.start_scoped_session(Some(account.person_id), session_id, context)
    }

    fn start_scoped_session(
        &mut self,
        person_id: Option<String>,
        session_id: impl Into<String>,
        context: WorkContext,
    ) -> Result<Session, StoreError> {
        let session_id = session_id.into();
        if session_id.trim().is_empty() {
            return Err(StoreError::Validation(
                "session_id must not be empty".to_owned(),
            ));
        }

        if let Some(person_id) = person_id.as_deref() {
            self.finish_idle_sessions(person_id);
        }
        if let Some(index) =
            self.state.sessions.iter().position(|session| {
                session.session_id == session_id && session.person_id == person_id
            })
        {
            // A push-created row is empty; the first real context fills it, as on PostgreSQL.
            let existing = &self.state.sessions[index];
            if existing.context == WorkContext::default() && context != WorkContext::default() {
                self.state.sessions[index].context = redact_work_context(&context);
                self.persist()?;
            }
            return Ok(self.state.sessions[index].clone());
        }

        // A session has no sensitivity field. Redact the human-readable lists
        // now. A later empty capture classifies that stored text, not the
        // original secret, so the observation label is not raised by a secret
        // that existed only here. The secret bytes are not stored.
        let context = redact_work_context(&context);
        let session = Session {
            session_id,
            person_id,
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
            is_admin: true,
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
            is_admin: false,
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

    /// Admin web session only. A device token fails with `WEB_ONLY_ERROR`, a non-admin
    /// session with `ADMIN_ONLY_ERROR`.
    pub fn authenticate_admin(&self, token: &str) -> Result<Account, StoreError> {
        let account = self.authenticate_session(token)?;
        if account.is_admin {
            Ok(account)
        } else {
            Err(StoreError::Validation(ADMIN_ONLY_ERROR.to_owned()))
        }
    }

    pub fn register_account_as_admin(
        &mut self,
        token: &str,
        email: &str,
        password: &str,
    ) -> Result<AuthSessionResult, StoreError> {
        self.authenticate_admin(token)?;
        self.register_account(email, password)
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

    /// Agent credential: a web session token or an active device token, resolved to the
    /// owning account.
    pub fn authenticate_account(&self, token: &str) -> Result<Account, StoreError> {
        self.authenticate_principal(token)
            .map(|(account, _)| account)
    }

    /// Web-only credential: a device token fails with `WEB_ONLY_ERROR`.
    pub fn authenticate_session(&self, token: &str) -> Result<Account, StoreError> {
        self.session_account(token).map_err(|error| {
            if self.is_device_token(token) {
                StoreError::Validation(WEB_ONLY_ERROR.to_owned())
            } else {
                error
            }
        })
    }

    /// True for any device token, including a revoked one.
    pub fn is_device_token(&self, token: &str) -> bool {
        let token_hash = hash_token(token);
        self.state
            .devices
            .iter()
            .any(|record| record.token_hash == token_hash)
    }

    /// Records device use in memory only, at most once per `LAST_SEEN_THROTTLE_SECS`; the next
    /// store write persists it.
    pub fn touch_device(&mut self, token: &str) {
        let token_hash = hash_token(token);
        if let Some(record) = self.state.devices.iter_mut().find(|record| {
            record.token_hash == token_hash && record.device.status == DeviceStatus::Active
        }) {
            touch_last_seen(&mut record.device);
        }
    }

    /// The account and whether the credential was a device token.
    fn authenticate_principal(&self, token: &str) -> Result<(Account, bool), StoreError> {
        let token_hash = hash_token(token);
        let Some(record) = self
            .state
            .devices
            .iter()
            .find(|record| record.token_hash == token_hash)
        else {
            return self.session_account(token).map(|account| (account, false));
        };
        if record.device.status != DeviceStatus::Active {
            return Err(StoreError::Validation("device is revoked".to_owned()));
        }
        self.state
            .accounts
            .iter()
            .find(|candidate| candidate.account.person_id == record.device.person_id)
            .map(|candidate| candidate.account.clone())
            .filter(|account| account.status == AccountStatus::Active)
            .map(|account| (account, true))
            .ok_or_else(|| StoreError::Validation("account authentication failed".to_owned()))
    }

    fn project_has_other_members(&self, project_id: &str, owner_person_id: &str) -> bool {
        self.state.project_members.iter().any(|member| {
            member.project_id == project_id && member.member_person_id != owner_person_id
        })
    }

    fn session_account(&self, token: &str) -> Result<Account, StoreError> {
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
        if self.is_device_token(token) {
            return Err(StoreError::Validation(WEB_ONLY_ERROR.to_owned()));
        }
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

    /// Admin web session only: every account, oldest first.
    pub fn accounts(&self, token: &str) -> Result<Vec<Account>, StoreError> {
        self.authenticate_admin(token)?;
        Ok(self
            .state
            .accounts
            .iter()
            .map(|record| record.account.clone())
            .collect())
    }

    /// Admin web session only. Sets another account's password and revokes every web
    /// session of that account; device tokens stay valid. Audited under the target.
    pub fn reset_account_password(
        &mut self,
        token: &str,
        person_id: &str,
        new_password: &str,
    ) -> Result<(), StoreError> {
        let admin = self.authenticate_admin(token)?;
        validate_password(new_password)?;
        let password_hash = hash_password(new_password)?;
        let record = self
            .state
            .accounts
            .iter_mut()
            .find(|record| record.account.person_id == person_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown account: {person_id}")))?;
        record.password_hash = password_hash;
        self.revoke_sessions_except(person_id, None);
        self.record_share_audit(
            person_id,
            "account_password_reset",
            Some(person_id),
            serde_json::json!({ "actor_person_id": admin.person_id }),
        );
        self.persist()
    }

    /// Web session only. Needs the current password, then revokes the account's other
    /// web sessions and keeps this one.
    pub fn change_password(
        &mut self,
        token: &str,
        current_password: &str,
        new_password: &str,
    ) -> Result<(), StoreError> {
        let account = self.authenticate_session(token)?;
        let record = self
            .state
            .accounts
            .iter_mut()
            .find(|record| record.account.person_id == account.person_id)
            .ok_or_else(|| StoreError::Validation("account authentication failed".to_owned()))?;
        if !verify_password(current_password, &record.password_hash) {
            return Err(StoreError::Validation(
                "invalid account credentials".to_owned(),
            ));
        }
        validate_password(new_password)?;
        record.password_hash = hash_password(new_password)?;
        self.revoke_sessions_except(&account.person_id, Some(&hash_token(token)));
        self.record_share_audit(
            &account.person_id,
            "account_password_change",
            Some(&account.person_id),
            serde_json::json!({}),
        );
        self.persist()
    }

    /// Stages the revocation of the person's open web sessions, except the one `keep`.
    fn revoke_sessions_except(&mut self, person_id: &str, keep: Option<&str>) {
        let revoked_at = timestamp();
        for session in &mut self.state.auth_sessions {
            if session.person_id == person_id
                && session.revoked_at.is_none()
                && keep != Some(session.token_hash.as_str())
            {
                session.revoked_at = Some(revoked_at.clone());
            }
        }
    }

    pub fn register_project(
        &mut self,
        account_token: &str,
        repository: &str,
        name: &str,
        default_branch: Option<String>,
    ) -> Result<Project, StoreError> {
        self.ensure_project(account_token, repository, name, default_branch)
            .map(|(project, _)| project)
    }

    /// `cairn init`: the caller's project for the repository, created when there is none.
    /// `name` defaults to the repository; the flag is true when the project is new.
    pub fn init_project(
        &mut self,
        account_token: &str,
        repository: &str,
        name: Option<&str>,
    ) -> Result<(Project, bool), StoreError> {
        let repository = repository.trim();
        validate_project_text(repository, "repository")?;
        let name = name.map(str::trim).filter(|name| !name.is_empty());
        self.ensure_project(account_token, repository, name.unwrap_or(repository), None)
    }

    fn ensure_project(
        &mut self,
        account_token: &str,
        repository: &str,
        name: &str,
        default_branch: Option<String>,
    ) -> Result<(Project, bool), StoreError> {
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
            return Ok((project, false));
        }
        let project = self.stage_project(&account.person_id, repository, name, default_branch);
        self.persist()?;
        Ok((project, true))
    }

    /// Adds a project owned by `person_id`. The caller persists.
    fn stage_project(
        &mut self,
        person_id: &str,
        repository: &str,
        name: &str,
        default_branch: Option<String>,
    ) -> Project {
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
        self.claim_project_owner(person_id, &project.project_id);
        project
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
        let account = self.authenticate_account(account_token)?;
        self.require_owned_project(&account, project_id)?;
        validate_project_text(name, "branch name")?;
        if let Some(branch) = self
            .state
            .project_branches
            .iter()
            .find(|branch| branch.project_id == project_id && branch.name == name.trim())
        {
            return Ok(branch.clone());
        }
        let branch = self.stage_branch(project_id, name, base_branch, base_commit, head_commit);
        self.persist()?;
        Ok(branch)
    }

    /// Adds an active branch. The caller persists and has checked it is not registered.
    fn stage_branch(
        &mut self,
        project_id: &str,
        name: &str,
        base_branch: Option<String>,
        base_commit: Option<String>,
        head_commit: Option<String>,
    ) -> ProjectBranch {
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
        branch
    }

    /// Membership through the committed project file: a capture whose `project_id` names an
    /// existing project that another person owns makes the person a member, idempotently.
    /// True when the person is such a member. An unknown, ownerless or own project id is
    /// ignored (false), so the capture falls back to repository registration. The caller
    /// has already acknowledged the capture, so it logs a failure.
    fn join_context_project(
        &mut self,
        person_id: &str,
        context: &WorkContext,
    ) -> Result<bool, StoreError> {
        let Some(project_id) = context
            .project_id
            .as_deref()
            .map(str::trim)
            .filter(|project_id| !project_id.is_empty())
        else {
            return Ok(false);
        };
        let Some(owner) = self.project_owner_id(project_id) else {
            return Ok(false);
        };
        if owner == person_id
            || !self
                .state
                .projects
                .iter()
                .any(|project| project.project_id == project_id)
        {
            return Ok(false);
        }
        if self
            .state
            .project_members
            .iter()
            .any(|record| record.project_id == project_id && record.member_person_id == person_id)
        {
            return Ok(true);
        }
        self.state.project_members.push(ProjectMemberRecord {
            project_id: project_id.to_owned(),
            owner_person_id: owner.clone(),
            member_person_id: person_id.to_owned(),
            created_at: timestamp(),
        });
        self.record_share_audit(
            &owner,
            "project_member_joined",
            Some(person_id),
            serde_json::json!({
                "project_id": project_id,
                "member_person_id": person_id,
            }),
        );
        self.persist()?;
        Ok(true)
    }

    /// Automatic registration on account capture. The person's own project for the
    /// repository comes first, then one they are a member of; a project with no owner
    /// record is never claimed. Only when there is none, a project owned by this person is
    /// created. A branch is registered on a project the person owns (branch writes are
    /// owner-only); an empty or detached branch is skipped. Idempotent.
    fn auto_register_project(
        &mut self,
        person_id: &str,
        context: &WorkContext,
    ) -> Result<(), StoreError> {
        // A member reads the owner's project, so no private project is created for it.
        if self.join_context_project(person_id, context)? {
            return Ok(());
        }
        let Some(repository) = context
            .repository
            .as_deref()
            .map(str::trim)
            .filter(|repository| !repository.is_empty())
        else {
            return Ok(());
        };
        validate_project_text(repository, "repository")?;
        let branch = context
            .branch
            .as_deref()
            .map(str::trim)
            .filter(|branch| !branch.is_empty() && *branch != "HEAD");
        if let Some(branch) = branch {
            validate_project_text(branch, "branch name")?;
        }
        let candidates = || {
            self.state
                .projects
                .iter()
                .filter(|project| project.repository == repository)
        };
        let owned = candidates()
            .find(|project| {
                self.project_owner_id(&project.project_id).as_deref() == Some(person_id)
            })
            .map(|project| project.project_id.clone());
        let is_member = candidates().any(|project| {
            self.state.project_members.iter().any(|member| {
                member.project_id == project.project_id && member.member_person_id == person_id
            })
        });
        let mut changed = false;
        let project_id = match owned {
            Some(project_id) => project_id,
            // A member reads the owner's project; only the owner registers its branches.
            None if is_member => return Ok(()),
            None => {
                changed = true;
                self.stage_project(person_id, repository, repository, None)
                    .project_id
            }
        };
        if let Some(branch) = branch
            && !self
                .state
                .project_branches
                .iter()
                .any(|known| known.project_id == project_id && known.name == branch)
        {
            changed = true;
            self.stage_branch(&project_id, branch, None, None, None);
        }
        if changed { self.persist() } else { Ok(()) }
    }

    pub fn branches(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectBranch>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_owned_project(&account, project_id)?;
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
        let account = self.authenticate_account(account_token)?;
        self.require_owned_project(&account, project_id)?;
        validate_project_text(path, "worktree path")?;
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
        let account = self.authenticate_account(account_token)?;
        self.require_owned_project(&account, project_id)?;
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
        let account = self.authenticate_account(account_token)?;
        self.require_owned_project(&account, project_id)?;
        validate_project_text(&input.key, "entity key")?;
        validate_entity_kind(&input.kind)?;
        let project = self
            .state
            .projects
            .iter()
            .find(|project| project.project_id == project_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown project: {project_id}")))?;
        if let Some(memory_id) = &input.source_memory_id {
            self.reject_other_owner(&account, memory_id)?;
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
        let account = self.authenticate_account(account_token)?;
        self.require_owned_project(&account, project_id)?;
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
        let account = self.authenticate_account(account_token)?;
        self.require_owned_project(&account, project_id)?;
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
        let account = self.authenticate_account(account_token)?;
        self.require_owned_project(&account, project_id)?;
        let project_memories = self
            .project_rows(project_id)
            .into_iter()
            .filter(|memory| !self.memory_is_forgotten(memory) && !is_expired(&memory.lifecycle))
            .collect::<Vec<_>>();
        let owners = self.memory_owner_map();
        let mut hidden = superseded_targets(&project_memories, &self.state.forgotten_ids, &owners);
        let corrected =
            superseded_targets(&self.state.memories, &self.state.forgotten_ids, &owners);
        hidden.extend(stale_copies(&project_memories, &corrected));
        let superseded_memory_ids = project_memories
            .iter()
            .filter(|memory| hidden.contains(memory.memory_id.as_str()))
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
        let (account, via_device) = self.authenticate_principal(account_token)?;
        self.require_owned_project(&account, project_id)?;
        if via_device && self.project_has_other_members(project_id, &account.person_id) {
            return Err(StoreError::Validation(WEB_ONLY_ERROR.to_owned()));
        }
        validate_project_text(branch_id, "branch_id")?;
        validate_project_text(memory_id, "memory_id")?;
        self.reject_other_owner(&account, memory_id)?;
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
            // Owner-visible rows of this project only: a branch aliased to a foreign row by old
            // data is refused below, not read and not repaired.
            if branch.merge_commit.as_deref() == Some(commit)
                && let Some(merged_memory_id) = &branch.merge_memory_id
                && let Some(memory) = self.project_rows(project_id).into_iter().find(|memory| {
                    memory.memory_id == *merged_memory_id
                        && memory.promoted_from.as_deref() == Some(memory_id)
                })
            {
                return Ok(ProjectMergeResult { branch, memory });
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
        // Owner-visible rows only, as in `promote_memory`: a stored foreign row must not stand
        // in for this merge.
        let promoted = if let Some(existing) =
            self.project_rows(project_id).into_iter().find(|memory| {
                memory.promoted_from.as_deref() == Some(memory_id)
                    && !self.memory_is_forgotten(memory)
                    && !is_expired(&memory.lifecycle)
            }) {
            existing
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
            promoted.source_event_ids.clear();
            promoted.supersedes =
                self.prior_project_copy(project_id, promoted.supersedes.as_deref());
            promoted.promotion_reason = Some(redact_sensitive_content(reason));
            redact_memory_metadata(&mut promoted);
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
        let account = self.authenticate_account(account_token)?;
        self.require_owned_project(&account, project_id)?;
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
        self.require_project_owner(&account, project_id)?;
        self.reject_other_owner(&account, memory_id)?;
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
        // Owner-visible rows only: a stored row another account owns must not stand in for
        // this person's promotion.
        // A forgotten or expired copy is a tombstone: promote again into a new live copy.
        if let Some(existing) = self.project_rows(project_id).into_iter().find(|memory| {
            memory.promoted_from.as_deref() == Some(memory_id)
                && !self.memory_is_forgotten(memory)
                && !is_expired(&memory.lifecycle)
        }) {
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
        promoted.source_event_ids.clear();
        promoted.supersedes = self.prior_project_copy(project_id, promoted.supersedes.as_deref());
        promoted.promotion_reason = Some(redact_sensitive_content(reason));
        redact_memory_metadata(&mut promoted);
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
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        self.reject_other_owner(&account, memory_id)?;
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
        let live = self.live_superseded_targets();
        let mut memories = self.live_project_memories(project_id);
        for memory in &mut memories {
            memory.promoted_from = live_origin(
                memory,
                &self.state.memories,
                &self.state.forgotten_ids,
                &live,
            );
        }
        Ok(memories)
    }

    pub fn audit_events(
        &self,
        account_token: &str,
        limit: usize,
    ) -> Result<Vec<AuditEvent>, StoreError> {
        let account = self.authenticate_session(account_token)?;
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
        self.reject_other_owner(&account, memory_id)?;
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
        let superseded = self.live_superseded_targets();
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
                    && !superseded.contains(memory.memory_id.as_str())
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
                // A member never sees the personal memory the copy came from.
                memory: Memory {
                    promoted_from: None,
                    ..memory.clone()
                },
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

    /// The project's owner first, then its members by join time. Owner and members may read it.
    pub fn project_people(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectPerson>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_reader(&account, project_id)?;
        let email_of = |person_id: &str| {
            self.state
                .accounts
                .iter()
                .find(|record| record.account.person_id == person_id)
                .map(|record| record.account.email.clone())
                .ok_or_else(|| StoreError::Validation("unknown account email".to_owned()))
        };
        let project = self
            .state
            .projects
            .iter()
            .find(|project| project.project_id == project_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown project: {project_id}")))?;
        let owner_id = self
            .project_owner_id(project_id)
            .ok_or_else(|| StoreError::Validation("account does not own the project".to_owned()))?;
        let mut people = vec![ProjectPerson {
            email: email_of(&owner_id)?,
            person_id: owner_id,
            role: "owner".to_owned(),
            joined_at: project.created_at.clone(),
        }];
        let mut members = Vec::new();
        for record in self
            .state
            .project_members
            .iter()
            .filter(|record| record.project_id == project_id)
        {
            members.push(ProjectPerson {
                person_id: record.member_person_id.clone(),
                email: email_of(&record.member_person_id)?,
                role: "member".to_owned(),
                joined_at: record.created_at.clone(),
            });
        }
        members.sort_by(|left, right| {
            left.joined_at
                .cmp(&right.joined_at)
                .then(left.person_id.cmp(&right.person_id))
        });
        people.extend(members);
        Ok(people)
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
            .map(|share| ProjectMemoryShare {
                member_email: self
                    .state
                    .accounts
                    .iter()
                    .find(|record| record.account.person_id == share.member_person_id)
                    .map(|record| record.account.email.clone()),
                ..share_view(share)
            })
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

    fn record_observation_owner(&mut self, person_id: &str, event_id: &str) {
        if self
            .state
            .observation_owners
            .iter()
            .any(|record| record.event_id == event_id)
        {
            return;
        }
        self.state.observation_owners.push(ObservationOwnerRecord {
            event_id: event_id.to_owned(),
            person_id: person_id.to_owned(),
        });
    }

    fn observation_owned_by(&self, event_id: &str, person_id: &str) -> bool {
        self.state
            .observation_owners
            .iter()
            .any(|record| record.event_id == event_id && record.person_id == person_id)
    }

    /// Memory ids and event ids share one id space, and an owner record needs no row (the
    /// generated source id of an explicit memory has none). An id another person owns in
    /// either list is not free for this person's capture or sync push.
    fn id_owned_by_other(&self, id: &str, person_id: &str) -> bool {
        self.state
            .memory_owners
            .iter()
            .any(|record| record.memory_id == id && record.person_id != person_id)
            || self
                .state
                .observation_owners
                .iter()
                .any(|record| record.event_id == id && record.person_id != person_id)
    }

    fn item_owner(&self, item_id: &str) -> Option<String> {
        self.state
            .memory_owners
            .iter()
            .find(|record| record.memory_id == item_id)
            .map(|record| record.person_id.clone())
            .or_else(|| {
                self.state
                    .observation_owners
                    .iter()
                    .find(|record| record.event_id == item_id)
                    .map(|record| record.person_id.clone())
            })
    }

    fn reject_other_owner(&self, account: &Account, target_id: &str) -> Result<(), StoreError> {
        let Some(owner) = self.item_owner(target_id) else {
            return Ok(());
        };
        if owner == account.person_id {
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

    /// The team's members are the project's members (people who joined by capture).
    fn team_view(&self, team: &ProjectTeamRecord) -> Result<ProjectTeam, StoreError> {
        let mut members = Vec::new();
        for record in self
            .state
            .project_members
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
            members.push(ProjectTeamMember {
                person_id: record.member_person_id.clone(),
                email,
                created_at: record.created_at.clone(),
            });
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

    /// `require_project_owner`, and the project must have an owner record that is this
    /// account. A project with no owner record is not maintainable until the existing
    /// registration claims it, the same as account import.
    fn require_owned_project(&self, account: &Account, project_id: &str) -> Result<(), StoreError> {
        self.require_project_owner(account, project_id)?;
        if self.project_owner_id(project_id).is_none() {
            return Err(StoreError::Validation(
                "account does not own the project".to_owned(),
            ));
        }
        Ok(())
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

    /// The earlier project copy of the memory a promoted correction supersedes. The copy links
    /// to it instead of to the personal id, which a member must never see.
    fn prior_project_copy(&self, project_id: &str, corrected: Option<&str>) -> Option<String> {
        let corrected = corrected?;
        self.project_rows(project_id)
            .into_iter()
            .find(|memory| {
                memory.promoted_from.as_deref() == Some(corrected)
                    && !self.memory_is_forgotten(memory)
                    && !is_expired(&memory.lifecycle)
            })
            .map(|memory| memory.memory_id)
    }

    fn live_project_memories(&self, project_id: &str) -> Vec<Memory> {
        let rows = self.project_rows(project_id);
        let owners = self.memory_owner_map();
        let mut hidden = superseded_targets(&rows, &self.state.forgotten_ids, &owners);
        // A live correction of the personal origin hides its copies too, though the
        // correction itself is not a project row.
        let corrected =
            superseded_targets(&self.state.memories, &self.state.forgotten_ids, &owners);
        hidden.extend(stale_copies(&rows, &corrected));
        let mut memories = rows
            .iter()
            .filter(|memory| {
                !self.memory_is_forgotten(memory)
                    && !is_expired(&memory.lifecycle)
                    && !hidden.contains(memory.memory_id.as_str())
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

    /// The owner's current share policy for the project.
    pub fn share_policy(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<ProjectSharePolicy, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        Ok(self.project_share_policy(project_id))
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
        self.reject_other_owner(account, target_id)
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
            .iter_mut()
            .find(|record| record.device.device_id == device_id)
        {
            if existing.device.person_id != person_id {
                return Err(StoreError::Validation(format!(
                    "device already belongs to another person: {device_id}"
                )));
            }
            if existing.device.status == DeviceStatus::Revoked {
                return Err(StoreError::Validation(
                    "device is revoked; register a new device id".to_owned(),
                ));
            }
            // The token is shown once and stored hashed, so a repeat registration issues a new one.
            let token = device_token(&device_id);
            existing.token_hash = hash_token(&token);
            existing.device.last_seen_at = timestamp();
            let device = existing.device.clone();
            self.persist()?;
            return Ok(DeviceRegistration { device, token });
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
            token_hash: hash_token(&token),
        });
        self.persist()?;
        Ok(DeviceRegistration { device, token })
    }

    pub fn devices(&self) -> impl Iterator<Item = &Device> {
        self.state.devices.iter().map(|record| &record.device)
    }

    fn authenticate_device_in_memory(
        &mut self,
        token: &str,
    ) -> Result<(Device, String), StoreError> {
        let token_hash = hash_token(token);
        let record = self
            .state
            .devices
            .iter_mut()
            .find(|record| record.token_hash == token_hash)
            .ok_or_else(|| StoreError::Validation("device authentication failed".to_owned()))?;
        if record.device.status != DeviceStatus::Active {
            return Err(StoreError::Validation("device is revoked".to_owned()));
        }
        if !self.state.accounts.iter().any(|candidate| {
            candidate.account.person_id == record.device.person_id
                && candidate.account.status == AccountStatus::Active
        }) {
            return Err(StoreError::Validation(
                "account authentication failed".to_owned(),
            ));
        }
        let previous_seen = record.device.last_seen_at.clone();
        touch_last_seen(&mut record.device);
        let device = record.device.clone();
        Ok((device, previous_seen))
    }

    fn restore_device_seen(&mut self, device_id: &str, previous_seen: &str) {
        if let Some(record) = self
            .state
            .devices
            .iter_mut()
            .find(|record| record.device.device_id == device_id)
        {
            record.device.last_seen_at = previous_seen.to_owned();
        }
    }

    fn persist_or_restore_seen(
        &mut self,
        device_id: &str,
        previous_seen: &str,
    ) -> Result<(), StoreError> {
        if let Err(error) = self.persist() {
            self.restore_device_seen(device_id, previous_seen);
            return Err(error);
        }
        Ok(())
    }

    /// Read-only sync authentication: last-seen stays in memory until a write persists it.
    pub fn authenticate_device(&mut self, token: &str) -> Result<Device, StoreError> {
        let (device, _) = self.authenticate_device_in_memory(token)?;
        Ok(device)
    }

    pub fn revoke_device(
        &mut self,
        auth_token: &str,
        target_device_id: &str,
    ) -> Result<Device, StoreError> {
        let caller = self.authenticate_session(auth_token)?;
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
        let device = target.device.clone();
        self.persist()?;
        Ok(device)
    }

    pub fn rotate_device_token(
        &mut self,
        auth_token: &str,
        target_device_id: &str,
    ) -> Result<DeviceRegistration, StoreError> {
        let caller = self.authenticate_session(auth_token)?;
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
        target.token_hash = hash_token(&token);
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
        let (device, previous_seen) = self.authenticate_device_in_memory(auth_token)?;
        if self.id_owned_by_other(&input.event_id, &device.person_id) {
            self.persist_or_restore_seen(&device.device_id, &previous_seen)?;
            return Err(StoreError::Validation(format!(
                "event belongs to another person: {}",
                input.event_id
            )));
        }
        if let Some(existing) = self
            .state
            .observations
            .iter()
            .find(|observation| observation.event_id == input.event_id)
            .cloned()
        {
            if let Some((cursor, person_id)) = self
                .state
                .sync_events
                .iter()
                .find(|event| event.event_id == existing.event_id)
                .map(|event| (event.cursor, event.person_id.clone()))
            {
                // A duplicate is a persisted sync event for this person, not the event id alone.
                let foreign = person_id != device.person_id;
                self.persist_or_restore_seen(&device.device_id, &previous_seen)?;
                if foreign {
                    return Err(StoreError::Validation(format!(
                        "event belongs to another person: {}",
                        existing.event_id
                    )));
                }
                if !is_replay(&existing, &input) {
                    return Err(changed_replay(&input.event_id));
                }
                return Ok(SyncPushResult {
                    accepted: true,
                    duplicate: true,
                    cursor,
                    observation: existing,
                });
            }

            let event_id = existing.event_id.clone();
            if !self.observation_owned_by(&event_id, &device.person_id) {
                let foreign_owner = self
                    .state
                    .observation_owners
                    .iter()
                    .any(|record| record.event_id == event_id);
                self.persist_or_restore_seen(&device.device_id, &previous_seen)?;
                return Err(if foreign_owner {
                    StoreError::Validation(format!("event belongs to another person: {event_id}"))
                } else {
                    outside_sync_scope(&event_id)
                });
            }

            // Attach the stored observation, but only for the same event: changed text conflicts.
            if !is_replay(&existing, &input) {
                self.persist_or_restore_seen(&device.device_id, &previous_seen)?;
                return Err(changed_replay(&event_id));
            }
            let previous_cursor = self.state.next_sync_cursor;
            self.state.next_sync_cursor = self.state.next_sync_cursor.saturating_add(1);
            let cursor = self.state.next_sync_cursor;
            self.state.sync_events.push(SyncEvent {
                cursor,
                event_id: event_id.clone(),
                person_id: device.person_id.clone(),
                device_id: device.device_id.clone(),
                observation: existing.clone(),
            });
            if let Err(error) = self.persist() {
                // Drop the new sync row only. The stored observation and owner stay.
                self.state
                    .sync_events
                    .retain(|event| event.event_id != event_id);
                self.state.next_sync_cursor = previous_cursor;
                self.restore_device_seen(&device.device_id, &previous_seen);
                return Err(error);
            }
            return Ok(SyncPushResult {
                accepted: true,
                duplicate: false,
                cursor,
                observation: existing,
            });
        }

        let capture = match self.stage_observation(Some(device.person_id.as_str()), input) {
            Ok(capture) => capture,
            Err(error) => {
                self.persist_or_restore_seen(&device.device_id, &previous_seen)?;
                return Err(error);
            }
        };

        let event_id = capture.observation.event_id.clone();
        // The device person owns the new observation, so the account sees it like an observe.
        self.record_observation_owner(&device.person_id, &event_id);
        // The hook never starts a session, so a push creates the row from the pushed context
        // (as on PostgreSQL). An empty row takes the first real context; a real one is kept.
        let session_id = capture.observation.session_id.clone();
        let pushed_context = capture.observation.context.clone();
        let mut created_session = false;
        let mut filled_session = None;
        if let Some(session) = self.state.sessions.iter_mut().find(|session| {
            session.session_id == session_id
                && session.person_id.as_deref() == Some(device.person_id.as_str())
        }) {
            if session.context == WorkContext::default() && pushed_context != WorkContext::default()
            {
                filled_session = Some(std::mem::replace(&mut session.context, pushed_context));
            }
        } else {
            created_session = true;
            self.state.sessions.push(Session {
                session_id: session_id.clone(),
                person_id: Some(device.person_id.clone()),
                context: pushed_context,
                started_at: timestamp(),
                ended_at: None,
            });
        }
        let previous_cursor = self.state.next_sync_cursor;
        self.state.next_sync_cursor = self.state.next_sync_cursor.saturating_add(1);
        let cursor = self.state.next_sync_cursor;
        self.state.sync_events.push(SyncEvent {
            cursor,
            event_id: event_id.clone(),
            person_id: device.person_id.clone(),
            device_id: device.device_id.clone(),
            observation: capture.observation.clone(),
        });
        if let Err(error) = self.persist() {
            // Observation and sync event land in one rename. A failed rename leaves neither.
            self.state
                .sync_events
                .retain(|event| event.event_id != event_id);
            self.state.next_sync_cursor = previous_cursor;
            self.unstage_observation(&event_id);
            self.unstage_observation_owner(&event_id);
            if created_session {
                self.state.sessions.retain(|session| {
                    session.session_id != session_id
                        || session.person_id.as_deref() != Some(device.person_id.as_str())
                });
            }
            if let Some(previous) = filled_session
                && let Some(session) = self.state.sessions.iter_mut().find(|session| {
                    session.session_id == session_id
                        && session.person_id.as_deref() == Some(device.person_id.as_str())
                })
            {
                session.context = previous;
            }
            self.restore_device_seen(&device.device_id, &previous_seen);
            return Err(error);
        }
        // The push is acknowledged. Housekeeping failures must not turn it into an error.
        if let Err(error) =
            self.auto_register_project(&device.person_id, &capture.observation.context)
        {
            eprintln!("cairn: automatic project registration failed: {error}");
        }
        self.finish_idle_sessions(&device.person_id);
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
            .filter(|event| {
                event.person_id == device.person_id
                    && event.cursor > cursor
                    && !self.state.forgotten_ids.contains(&event.event_id)
            })
            .cloned()
            .collect();
        let deletions: Vec<SyncDeletion> = self
            .state
            .sync_deletions
            .iter()
            .filter(|deletion| deletion.person_id == device.person_id && deletion.cursor > cursor)
            .map(|deletion| SyncDeletion {
                cursor: deletion.cursor,
                item_id: deletion.item_id.clone(),
            })
            .collect();
        let next_cursor = events
            .iter()
            .map(|event| event.cursor)
            .chain(deletions.iter().map(|deletion| deletion.cursor))
            .max()
            .unwrap_or(cursor.max(device.cursor));
        if next_cursor > device.cursor
            && let Some(record) = self
                .state
                .devices
                .iter_mut()
                .find(|record| record.device.device_id == device.device_id)
        {
            record.device.cursor = next_cursor;
            self.persist()?;
        }
        Ok(SyncPullResult {
            cursor: next_cursor,
            events,
            deletions,
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

    fn stage_observation(
        &mut self,
        person_id: Option<&str>,
        input: ObservationInput,
    ) -> Result<CaptureResult, StoreError> {
        validate_observation(&input)?;

        if let Some(existing) = self
            .state
            .observations
            .iter()
            .find(|observation| observation.event_id == input.event_id)
        {
            if !is_replay(existing, &input) {
                return Err(changed_replay(&input.event_id));
            }
            return Ok(CaptureResult {
                accepted: true,
                duplicate: true,
                observation: existing.clone(),
            });
        }

        // A bare project id (the CLI sends only that) still inherits the session's context.
        let bare = WorkContext {
            project_id: None,
            ..input.context.clone()
        } == WorkContext::default();
        let mut context = if bare {
            self.state
                .sessions
                .iter()
                .find(|session| {
                    session.session_id == input.session_id
                        && session.person_id.as_deref() == person_id
                })
                .map(|session| session.context.clone())
                .unwrap_or_default()
        } else {
            input.context.clone()
        };
        context.project_id = input.context.project_id.clone().or(context.project_id);
        let mut normalized_input = input;
        normalized_input.context = context;
        let observation = normalize_observation(&normalized_input)?;
        self.state.observations.push(observation.clone());
        Ok(CaptureResult {
            accepted: true,
            duplicate: false,
            observation,
        })
    }

    fn unstage_observation(&mut self, event_id: &str) {
        self.state
            .observations
            .retain(|observation| observation.event_id != event_id);
    }

    fn unstage_observation_owner(&mut self, event_id: &str) {
        self.state
            .observation_owners
            .retain(|record| record.event_id != event_id);
    }

    pub fn capture_observation(
        &mut self,
        input: ObservationInput,
    ) -> Result<CaptureResult, StoreError> {
        let result = self.stage_observation(None, input)?;
        if result.duplicate {
            return Ok(result);
        }
        let event_id = result.observation.event_id.clone();
        if let Err(error) = self.persist() {
            // The rename did not replace the file. Drop the staged row so a retry writes.
            self.unstage_observation(&event_id);
            return Err(error);
        }
        Ok(result)
    }

    pub fn has_accounts(&self) -> bool {
        !self.state.accounts.is_empty()
    }

    pub fn capture_for_account(
        &mut self,
        account_token: &str,
        input: ObservationInput,
    ) -> Result<CaptureResult, StoreError> {
        let account = self.authenticate_account(account_token)?;
        // Auth before the event id. Do not return or claim a foreign or unowned row.
        validate_observation(&input)?;
        let person_id = account.person_id.clone();
        if self.id_owned_by_other(&input.event_id, &person_id) {
            return Err(StoreError::Validation(format!(
                "event belongs to another person: {}",
                input.event_id
            )));
        }
        if self
            .state
            .observations
            .iter()
            .any(|observation| observation.event_id == input.event_id)
            && !self.observation_owned_by(&input.event_id, &person_id)
        {
            let owned_by_someone_else = self
                .state
                .observation_owners
                .iter()
                .any(|record| record.event_id == input.event_id);
            return Err(if owned_by_someone_else {
                StoreError::Validation(format!(
                    "event belongs to another person: {}",
                    input.event_id
                ))
            } else {
                outside_sync_scope(&input.event_id)
            });
        }
        let result = self.stage_observation(Some(person_id.as_str()), input)?;
        if result.duplicate {
            return Ok(result);
        }
        let event_id = result.observation.event_id.clone();
        self.record_observation_owner(&person_id, &event_id);
        if let Err(error) = self.persist() {
            // Observation and owner land in one rename. A failed rename leaves neither.
            self.unstage_observation(&event_id);
            self.unstage_observation_owner(&event_id);
            return Err(error);
        }
        // The capture is acknowledged. Housekeeping failures must not turn it into an error.
        if let Err(error) = self.auto_register_project(&person_id, &result.observation.context) {
            eprintln!("cairn: automatic project registration failed: {error}");
        }
        self.finish_idle_sessions(&person_id);
        Ok(result)
    }

    /// Finishes this person's sessions that have retained observations, none newer than
    /// `IDLE_SESSION_SECS`, and no current summary: open ones, and ended ones whose id was
    /// reused for observations the summary does not cover. A session with no retained
    /// observations is skipped. Failures are logged; the trigger that called this has
    /// already succeeded.
    fn finish_idle_sessions(&mut self, person_id: &str) {
        let now = now_seconds();
        let retained = self.retained_by_session(person_id);
        let superseded = self.live_superseded_targets();
        let idle: Vec<String> = self
            .state
            .sessions
            .iter()
            .filter(|session| session.person_id.as_deref() == Some(person_id))
            .filter(|session| {
                let Some(observations) = retained.get(session.session_id.as_str()) else {
                    return false;
                };
                let newest = observations
                    .iter()
                    .map(|observation| observation.captured_at.parse::<u64>().unwrap_or(0))
                    .max()
                    .unwrap_or(0);
                now.saturating_sub(newest) > IDLE_SESSION_SECS
                    && self.session_needs_summary(person_id, session, observations, &superseded)
            })
            .map(|session| session.session_id.clone())
            .take(MAX_SWEEP_SESSIONS)
            .collect();
        drop((retained, superseded));
        self.finish_sessions(person_id, &idle);
    }

    /// Periodic close: the idle-session sweep for every person that has a session.
    pub fn sweep_idle_sessions(&mut self) {
        let people: HashSet<String> = self
            .state
            .sessions
            .iter()
            .filter_map(|session| session.person_id.clone())
            .collect();
        for person_id in people {
            self.finish_idle_sessions(&person_id);
        }
    }

    fn finish_sessions(&mut self, person_id: &str, session_ids: &[String]) {
        for session_id in session_ids {
            if let Err(error) = self.finish_session(Some(person_id.to_owned()), session_id) {
                eprintln!("cairn: automatic session close failed: {error}");
            }
        }
    }

    /// This person's retained observations grouped by session: owned, not forgotten, not
    /// expired. The same filter `finish_session_staged` summarises.
    fn retained_by_session(&self, person_id: &str) -> HashMap<&str, Vec<&Observation>> {
        let owned: HashSet<&str> = self
            .state
            .observation_owners
            .iter()
            .filter(|record| record.person_id == person_id)
            .map(|record| record.event_id.as_str())
            .collect();
        let mut grouped: HashMap<&str, Vec<&Observation>> = HashMap::new();
        for observation in &self.state.observations {
            if owned.contains(observation.event_id.as_str())
                && !self.state.forgotten_ids.contains(&observation.event_id)
                && !is_expired(&observation.lifecycle)
            {
                grouped
                    .entry(observation.session_id.as_str())
                    .or_default()
                    .push(observation);
            }
        }
        grouped
    }

    /// A summary the person hid is left alone: its id is forgotten, or a live correction
    /// supersedes it. Deriving it again under the same id would undo that action.
    fn session_needs_summary(
        &self,
        person_id: &str,
        session: &Session,
        observations: &[&Observation],
        superseded: &HashSet<&str>,
    ) -> bool {
        let memory_id = session_memory_id(Some(person_id), &session.session_id);
        let Some(summary) = self
            .state
            .memories
            .iter()
            .find(|memory| memory.memory_id == memory_id)
        else {
            return true;
        };
        !self.state.forgotten_ids.contains(&memory_id)
            && !superseded.contains(memory_id.as_str())
            && (session.ended_at.is_none()
                || summary_is_stale(summary, observations, &self.state.forgotten_ids))
    }

    pub fn end_session(&mut self, session_id: &str) -> Result<Memory, StoreError> {
        self.finish_session(None, session_id)
    }

    pub fn end_session_for_account(
        &mut self,
        account_token: &str,
        session_id: &str,
    ) -> Result<Memory, StoreError> {
        let account = self.authenticate_account(account_token)?;
        let person_id = account.person_id;
        self.finish_session(Some(person_id), session_id)
    }

    /// A refused end leaves the session open: `ended_at` is set before the checks that can fail.
    fn finish_session(
        &mut self,
        person_id: Option<String>,
        session_id: &str,
    ) -> Result<Memory, StoreError> {
        let finished = self.finish_session_staged(person_id, session_id);
        if finished.is_err() {
            self.state = self.persisted.clone();
        }
        finished
    }

    fn finish_session_staged(
        &mut self,
        person_id: Option<String>,
        session_id: &str,
    ) -> Result<Memory, StoreError> {
        if session_id.trim().is_empty() {
            return Err(StoreError::Validation(
                "session_id must not be empty".to_owned(),
            ));
        }

        let memory_id = session_memory_id(person_id.as_deref(), session_id);
        let hidden = self.state.forgotten_ids.contains(&memory_id)
            || self.live_superseded_targets().contains(memory_id.as_str());
        let owned_events = person_id.as_ref().map(|person_id| {
            self.state
                .observation_owners
                .iter()
                .filter(|record| record.person_id == *person_id)
                .map(|record| record.event_id.clone())
                .collect::<HashSet<_>>()
        });
        let session = self
            .state
            .sessions
            .iter_mut()
            .find(|session| session.session_id == session_id && session.person_id == person_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown session: {session_id}")))?;
        if session.ended_at.is_none() {
            session.ended_at = Some(timestamp());
        }

        let observations: Vec<Observation> = self
            .state
            .observations
            .iter()
            .filter(|observation| {
                let in_scope = match owned_events.as_ref() {
                    Some(events) => events.contains(&observation.event_id),
                    None => !self
                        .state
                        .observation_owners
                        .iter()
                        .any(|record| record.event_id == observation.event_id),
                };
                observation.session_id == session_id
                    && !self.state.forgotten_ids.contains(&observation.event_id)
                    && !is_expired(&observation.lifecycle)
                    && in_scope
            })
            .cloned()
            .collect();

        if observations.is_empty() {
            return Err(StoreError::Validation(format!(
                "session has no retained observations: {session_id}"
            )));
        }

        let mut earlier = None;
        if let Some(memory) = self
            .state
            .memories
            .iter()
            .find(|memory| memory.memory_id == memory_id)
            .cloned()
        {
            if hidden {
                // Forgotten or corrected by the person: respect it, do not derive it again.
                return Err(StoreError::Validation(format!(
                    "session summary was forgotten or superseded: {memory_id}"
                )));
            }
            let refs: Vec<&Observation> = observations.iter().collect();
            if !summary_is_stale(&memory, &refs, &self.state.forgotten_ids) {
                self.persist()?;
                return Ok(memory);
            }
            // The session id was reused after its summary, or a source was forgotten:
            // derive the summary again from what is retained.
            self.state
                .memories
                .retain(|existing| existing.memory_id != memory_id);
            earlier = Some(memory);
        }

        let mut memory = derive_session_memory(
            person_id.as_deref(),
            session_id,
            &session.context,
            &observations,
        )?;
        if let Some(earlier) = &earlier {
            carry_summary_state(earlier, &mut memory);
        }
        self.state.memories.push(memory.clone());
        if let Some(person_id) = person_id.as_deref() {
            self.record_memory_owner(person_id, &memory.memory_id);
        }
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

        let memory = self.stage_explicit_memory(content, concepts, context, lifecycle, kind)?;
        self.persist()?;
        Ok(memory)
    }

    /// The validation and row of `remember_with_kind`, staged without persisting.
    fn stage_explicit_memory(
        &mut self,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
        lifecycle: LifecycleInput,
        kind: MemoryKind,
    ) -> Result<Memory, StoreError> {
        let memory = build_explicit_memory_record(content, concepts, context, lifecycle, kind)?;
        self.state.memories.push(memory.clone());
        Ok(memory)
    }

    /// Own the source id this store just generated for `memory`, so a forget can cascade it
    /// and no other account can take it. Only generated ids: a source id named by imported
    /// metadata is never recorded.
    fn record_generated_source_owners(&mut self, person_id: &str, memory: &Memory) {
        for source in &memory.source_event_ids {
            self.record_observation_owner(person_id, source);
        }
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
        // The row and its owner records land in one rename; a failed rename leaves none.
        let memory = self.stage_explicit_memory(content, concepts, context, lifecycle, kind)?;
        self.record_memory_owner(&account.person_id, &memory.memory_id);
        self.record_generated_source_owners(&account.person_id, &memory);
        if let Err(error) = self.persist() {
            self.unstage_memory(&memory);
            return Err(error);
        }
        Ok(memory)
    }

    /// Remove a staged generated memory with its owner records, so a failed rename leaves none.
    fn unstage_memory(&mut self, memory: &Memory) {
        self.state
            .memories
            .retain(|stored| stored.memory_id != memory.memory_id);
        self.state
            .memory_owners
            .retain(|record| record.memory_id != memory.memory_id);
        self.state
            .observation_owners
            .retain(|record| !memory.source_event_ids.contains(&record.event_id));
    }

    pub fn correct(
        &mut self,
        target_id: &str,
        content: &str,
        concepts: Vec<String>,
        context: WorkContext,
        lifecycle: LifecycleInput,
    ) -> Result<Memory, StoreError> {
        let memory = self.stage_correction(target_id, content, concepts, context, lifecycle)?;
        if let Err(error) = self.persist() {
            self.unstage_memory(&memory);
            return Err(error);
        }
        Ok(memory)
    }

    /// The validation and row of `correct`, staged without persisting.
    fn stage_correction(
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
        let inherited = context == WorkContext::default();
        let source_context = if inherited {
            target.1.clone()
        } else {
            context.clone()
        };
        let mut lifecycle = Lifecycle::explicit(&lifecycle, content, &concepts, &source_context);
        lifecycle.confidence = 100;
        let memory = Memory {
            memory_id: format!("memory-correction-{suffix}"),
            session_id: format!("session-correction-{suffix}"),
            content: build_explicit_memory(content, concepts),
            source_event_ids: vec![correction_event],
            context: redact_work_context(&source_context),
            lifecycle,
            kind: MemoryKind::Correction,
            scope: if inherited {
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
        // The row and its owner records land in one rename; a failed rename leaves none.
        let memory = self.stage_correction(target_id, content, concepts, context, lifecycle)?;
        self.record_memory_owner(&account.person_id, &memory.memory_id);
        self.record_generated_source_owners(&account.person_id, &memory);
        if let Err(error) = self.persist() {
            self.unstage_memory(&memory);
            return Err(error);
        }
        Ok(memory)
    }

    pub fn export(&self) -> ExportBundle {
        self.export_owned(None)
    }

    pub fn export_for_account(&self, account_token: &str) -> Result<ExportBundle, StoreError> {
        let account = self.authenticate_account(account_token)?;
        Ok(self.export_owned(Some(account.person_id.as_str())))
    }

    fn export_owned(&self, person_id: Option<&str>) -> ExportBundle {
        let observations = self
            .state
            .observations
            .iter()
            .filter(|observation| {
                person_id.is_none_or(|person_id| {
                    self.observation_owned_by(&observation.event_id, person_id)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        let memories = self
            .state
            .memories
            .iter()
            .filter(|memory| {
                person_id.is_none_or(|person_id| self.memory_owned_by(&memory.memory_id, person_id))
            })
            .cloned()
            .collect::<Vec<_>>();
        let sessions = self
            .state
            .sessions
            .iter()
            .filter(|session| session.person_id.as_deref() == person_id)
            .cloned()
            .collect();
        let mut forgotten_ids = self
            .state
            .forgotten_ids
            .iter()
            .filter(|id| {
                person_id.is_none_or(|person_id| {
                    self.observation_owned_by(id, person_id)
                        || self.memory_owned_by(id, person_id)
                        || memories.iter().any(|memory| {
                            memory.source_event_ids.iter().any(|source| source == *id)
                        })
                        || observations
                            .iter()
                            .any(|observation| observation.event_id == **id)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        forgotten_ids.sort();
        ExportBundle {
            format: EXPORT_FORMAT.to_owned(),
            version: EXPORT_VERSION,
            exported_at: timestamp(),
            sessions,
            observations,
            memories,
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

    pub fn import_for_account(
        &mut self,
        account_token: &str,
        mut bundle: ExportBundle,
    ) -> Result<ImportResult, StoreError> {
        let account = self.authenticate_account(account_token)?;
        let person_id = account.person_id.clone();
        for observation in &bundle.observations {
            if self
                .item_owner(&observation.event_id)
                .is_some_and(|owner| owner != person_id)
            {
                return Err(StoreError::Validation(
                    "account does not own the memory".to_owned(),
                ));
            }
        }
        for memory in &bundle.memories {
            if self
                .item_owner(&memory.memory_id)
                .is_some_and(|owner| owner != person_id)
            {
                return Err(StoreError::Validation(
                    "account does not own the memory".to_owned(),
                ));
            }
        }
        // A project-linked row must name a project this person owns. A missing, foreign or
        // unowned project gets one error, so it does not say which. Only memories carry a
        // project id; the bundle holds no project records, so a cross-store restore needs the
        // matching project registered first.
        for memory in &bundle.memories {
            if memory.project_id.as_deref().is_some_and(|project_id| {
                self.project_owner_id(project_id).as_deref() != Some(person_id.as_str())
            }) {
                return Err(StoreError::Validation(
                    "account does not own the project".to_owned(),
                ));
            }
        }
        let observation_ids = bundle
            .observations
            .iter()
            .map(|observation| observation.event_id.clone())
            .collect::<Vec<_>>();
        let memory_ids = bundle
            .memories
            .iter()
            .map(|memory| memory.memory_id.clone())
            .collect::<Vec<_>>();
        // A tombstone applies only to an id this person owns or brings as a row. A foreign
        // id rejects the import with the row-ownership error (no payload or owner), which
        // does reveal that the id is owned; an unknown id is ignored. A source id is not
        // proof, since a row can name a foreign or future id as its source. A caller can
        // still bring a row under an unused id and so reserve it; capture and sync push
        // then reject that id loudly (see `id_owned_by_other`).
        let brought = observation_ids
            .iter()
            .chain(&memory_ids)
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut applied = Vec::with_capacity(bundle.forgotten_ids.len());
        for id in std::mem::take(&mut bundle.forgotten_ids) {
            match self.item_owner(&id) {
                Some(owner) if owner != person_id => {
                    return Err(StoreError::Validation(
                        "account does not own the memory".to_owned(),
                    ));
                }
                Some(_) => applied.push(id),
                None if brought.contains(id.as_str()) => applied.push(id),
                None => {}
            }
        }
        bundle.forgotten_ids = applied;
        for session in &mut bundle.sessions {
            if session
                .person_id
                .as_deref()
                .is_some_and(|owner| owner != person_id)
            {
                return Err(StoreError::Validation(
                    "account does not own the memory".to_owned(),
                ));
            }
            // An old bundle has no person. Bind it to the caller so it does not become the unscoped row.
            session.person_id = Some(person_id.clone());
        }
        // One persist for rows, tombstones, expiry tombstones and owners: a failure restores
        // the prior state, so no row is left in memory or on disk without its owner.
        let (candidate, result, _) = self.stage_import(bundle)?;
        self.state = candidate;
        for event_id in observation_ids {
            self.record_observation_owner(&person_id, &event_id);
        }
        for memory_id in memory_ids {
            self.record_memory_owner(&person_id, &memory_id);
        }
        self.persist()?;
        Ok(result)
    }

    pub fn import_bundle(&mut self, bundle: ExportBundle) -> Result<ImportResult, StoreError> {
        let (candidate, result, changed) = self.stage_import(bundle)?;
        if changed {
            self.state = candidate;
            self.persist()?;
        }
        Ok(result)
    }

    /// Merges a bundle into a clone of the state, with expiry tombstones when anything changed.
    /// Nothing is written; the caller commits the candidate.
    fn stage_import(
        &self,
        mut bundle: ExportBundle,
    ) -> Result<(StoreState, ImportResult, bool), StoreError> {
        normalize_export_bundle(&mut bundle)?;
        let mut candidate = self.state.clone();
        let mut result = ImportResult {
            imported_sessions: 0,
            imported_observations: 0,
            imported_memories: 0,
            imported_forgotten_ids: 0,
        };

        for session in bundle.sessions {
            if let Some(existing) = candidate.sessions.iter().find(|existing| {
                existing.session_id == session.session_id && existing.person_id == session.person_id
            }) {
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

        let changed = result.imported_sessions > 0
            || result.imported_observations > 0
            || result.imported_memories > 0
            || result.imported_forgotten_ids > 0;
        if changed {
            sweep_expired_state(&mut candidate);
        }
        Ok((candidate, result, changed))
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
        let mut results = self.ranked_matches(query, filter)?;
        prefer_newer_contradiction(&mut results, &self.state.memories);
        results.truncate(limit);
        Ok(results)
    }

    /// Score-ranked matches before the contradiction tiebreak and the limit, so an account
    /// read can drop foreign ids first and break ties only on what it may see.
    fn ranked_matches(
        &self,
        query: &str,
        filter: &RecallFilter,
    ) -> Result<Vec<RecallResult>, StoreError> {
        if query.trim().is_empty() {
            return Err(StoreError::Validation("query must not be empty".to_owned()));
        }

        let terms = tokenize(query);
        let superseded = self.live_superseded_targets();
        let mut results: Vec<RecallResult> = self
            .state
            .memories
            .iter()
            .filter(|memory| self.memory_is_recalled(memory, &superseded))
            .filter(|memory| memory_matches_filter(memory, filter))
            .filter_map(|memory| {
                let score = score_memory(memory, &terms);
                (score > 0.0).then(|| {
                    recall_result_with(
                        memory,
                        score,
                        &self.state.memories,
                        &self.state.forgotten_ids,
                        filter,
                        &superseded,
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
        prefer_decision(&mut results, &self.state.memories);
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

    fn retain_account_recall(&self, person_id: &str, results: &mut Vec<RecallResult>) {
        // Keep this account's rows, then drop contradiction ids it does not own.
        // Callers truncate or pack after this, so a foreign id cannot survive the limit.
        results.retain(|result| self.memory_owned_by(&result.memory_id, person_id));
        for result in results {
            result
                .contradiction_ids
                .retain(|id| self.memory_owned_by(id, person_id));
            result.contradictory = !result.contradiction_ids.is_empty();
        }
    }

    pub fn search_for_account(
        &self,
        account_token: &str,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<Vec<RecallResult>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        let mut results = self.ranked_matches(query, filter)?;
        self.retain_account_recall(&account.person_id, &mut results);
        prefer_newer_contradiction(&mut results, &self.state.memories);
        results.truncate(limit);
        Ok(results)
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

    pub fn resume_for_account(
        &self,
        account_token: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, StoreError> {
        let account = self.authenticate_account(account_token)?;
        let mut results = self.resume_context(usize::MAX, filter)?;
        self.retain_account_recall(&account.person_id, &mut results.results);
        Ok(build_context_pack(
            "resume".to_owned(),
            &mut results.results,
            limit,
        ))
    }

    pub fn unresolved_for_account(
        &self,
        account_token: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, StoreError> {
        let account = self.authenticate_account(account_token)?;
        let mut results = self.unresolved_context(usize::MAX, filter)?;
        self.retain_account_recall(&account.person_id, &mut results.results);
        Ok(build_context_pack(
            "unresolved".to_owned(),
            &mut results.results,
            limit,
        ))
    }

    pub fn why_changed_for_account(
        &self,
        account_token: &str,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, StoreError> {
        let mut results = self.search_for_account(account_token, query, usize::MAX, filter)?;
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

    pub fn resume_context(
        &self,
        limit: usize,
        filter: &RecallFilter,
    ) -> Result<ContextPack, StoreError> {
        let superseded = self.live_superseded_targets();
        let mut results: Vec<RecallResult> = self
            .state
            .memories
            .iter()
            .rev()
            .filter(|memory| memory.kind == MemoryKind::SessionSummary)
            .filter(|memory| self.memory_is_recalled(memory, &superseded))
            .filter(|memory| memory_matches_filter(memory, filter))
            .map(|memory| {
                recall_result_with(
                    memory,
                    1.0,
                    &self.state.memories,
                    &self.state.forgotten_ids,
                    filter,
                    &superseded,
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
        let superseded = self.live_superseded_targets();
        let mut results: Vec<RecallResult> = self
            .state
            .memories
            .iter()
            .rev()
            .filter(|memory| memory.kind == MemoryKind::SessionSummary)
            .filter(|memory| !memory.unresolved_work.is_empty())
            .filter(|memory| self.memory_is_recalled(memory, &superseded))
            .filter(|memory| memory_matches_filter(memory, filter))
            .map(|memory| {
                recall_result_with(
                    memory,
                    1.0,
                    &self.state.memories,
                    &self.state.forgotten_ids,
                    filter,
                    &superseded,
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
        memory_forgotten(memory, &self.state.forgotten_ids)
    }

    /// Ids a live successor hides, built once per request. The owner map is the only
    /// place file ownership enters supersession; see `superseded_targets`.
    fn live_superseded_targets(&self) -> HashSet<&str> {
        let mut hidden = superseded_targets(
            &self.state.memories,
            &self.state.forgotten_ids,
            &self.memory_owner_map(),
        );
        hidden.extend(stale_copies(&self.state.memories, &hidden.clone()));
        hidden
    }

    fn memory_owner_map(&self) -> HashMap<&str, &str> {
        let mut owners = HashMap::with_capacity(self.state.memory_owners.len());
        for record in &self.state.memory_owners {
            // First record wins, like `item_owner`.
            owners
                .entry(record.memory_id.as_str())
                .or_insert(record.person_id.as_str());
        }
        owners
    }

    /// Rows with this project id that belong in the owner's project views: rows the project
    /// owner owns, and rows with no owner (local or pre-account data). A row another person
    /// owns is stored but not shown, so it cannot appear in or hide anything in the owner's
    /// list, share candidates, conflicts or maintenance. Import cannot add one now; old data
    /// may hold one. Nothing is deleted.
    fn project_rows(&self, project_id: &str) -> Vec<Memory> {
        let project_owner = self.project_owner_id(project_id);
        let owners = self.memory_owner_map();
        self.state
            .memories
            .iter()
            .filter(|memory| {
                memory.project_id.as_deref() == Some(project_id)
                    && owners
                        .get(memory.memory_id.as_str())
                        .is_none_or(|owner| project_owner.as_deref().is_none_or(|p| p == *owner))
            })
            .cloned()
            .collect()
    }

    fn memory_is_recalled(&self, memory: &Memory, superseded: &HashSet<&str>) -> bool {
        !self.memory_is_forgotten(memory)
            && !is_expired(&memory.lifecycle)
            && !superseded.contains(memory.memory_id.as_str())
    }

    /// Writes the state. Every mutator commits through here, so a failed write restores the
    /// last persisted state and memory never runs ahead of the disk.
    fn persist(&mut self) -> Result<(), StoreError> {
        let written = self.write_state();
        match written {
            Ok(()) => self.persisted = self.state.clone(),
            Err(_) => self.state = self.persisted.clone(),
        }
        written
    }

    fn write_state(&self) -> Result<(), StoreError> {
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
        let sources = self.hide_id(id, None)?;
        self.note_sync_deletions(None, id, &sources);
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
        let person_id = account.person_id.clone();
        // Only an id this person owns. An unowned legacy id reads like an unknown one.
        if self.item_owner(id).as_deref() != Some(person_id.as_str()) {
            return Err(StoreError::Validation(format!(
                "unknown memory or event: {id}"
            )));
        }
        let sources = self.hide_id(id, Some(&person_id))?;
        self.note_sync_deletions(Some(&person_id), id, &sources);
        self.record_share_audit(&person_id, action, Some(id), serde_json::json!({}));
        self.persist()
    }

    fn source_ids(&self, id: &str) -> Vec<String> {
        self.state
            .memories
            .iter()
            .find(|memory| memory.memory_id == id)
            // A promoted copy never cascades into its origin's events.
            .filter(|memory| memory.promoted_from.is_none())
            .map(|memory| memory.source_event_ids.clone())
            .unwrap_or_default()
    }

    fn note_sync_deletions(&mut self, person_id: Option<&str>, item_id: &str, sources: &[String]) {
        let persons = match person_id {
            Some(person_id) => vec![person_id.to_owned()],
            None => {
                let mut persons = self.persons_for_item(item_id);
                for source in sources {
                    for person in self.persons_for_item(source) {
                        if !persons.contains(&person) {
                            persons.push(person);
                        }
                    }
                }
                persons
            }
        };
        for person in persons {
            self.push_sync_deletion(&person, item_id);
            for source in sources {
                self.push_sync_deletion(&person, source);
            }
        }
    }

    fn persons_for_item(&self, item_id: &str) -> Vec<String> {
        let mut persons = Vec::new();
        for event in &self.state.sync_events {
            if event.event_id == item_id && !persons.contains(&event.person_id) {
                persons.push(event.person_id.clone());
            }
        }
        if let Some(owner) = self.item_owner(item_id)
            && !persons.contains(&owner)
        {
            persons.push(owner);
        }
        persons
    }

    fn push_sync_deletion(&mut self, person_id: &str, item_id: &str) {
        if person_id.is_empty() || item_id.is_empty() {
            return;
        }
        if self
            .state
            .sync_deletions
            .iter()
            .any(|deletion| deletion.person_id == person_id && deletion.item_id == item_id)
        {
            return;
        }
        self.state.next_sync_cursor = self.state.next_sync_cursor.saturating_add(1);
        self.state.sync_deletions.push(SyncDeletionRecord {
            cursor: self.state.next_sync_cursor,
            person_id: person_id.to_owned(),
            item_id: item_id.to_owned(),
        });
    }

    /// Tombstones `id` and returns the source ids of that memory it tombstoned too. Trusted
    /// unscoped calls (`owner` is `None`) cascade every source id. An account cascades only
    /// the sources it owns: a source id in a row's metadata is never authority over another
    /// person's or a future id.
    fn hide_id(&mut self, id: &str, owner: Option<&str>) -> Result<Vec<String>, StoreError> {
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
        let sources = self
            .source_ids(id)
            .into_iter()
            .filter(|source| {
                owner.is_none_or(|person_id| self.item_owner(source).as_deref() == Some(person_id))
            })
            .collect::<Vec<_>>();
        self.state.forgotten_ids.extend(sources.iter().cloned());
        Ok(sources)
    }

    pub fn lifecycle_summary(&self) -> LifecycleSummary {
        self.summarize_lifecycle(None)
    }

    pub fn lifecycle_summary_for_account(
        &self,
        account_token: &str,
    ) -> Result<LifecycleSummary, StoreError> {
        let account = self.authenticate_account(account_token)?;
        Ok(self.summarize_lifecycle(Some(account.person_id.as_str())))
    }

    /// Counts every row, or only rows the person owns.
    fn summarize_lifecycle(&self, person_id: Option<&str>) -> LifecycleSummary {
        let observations = self.state.observations.iter().filter(|observation| {
            person_id
                .is_none_or(|person_id| self.observation_owned_by(&observation.event_id, person_id))
        });
        let memories = self.state.memories.iter().filter(|memory| {
            person_id.is_none_or(|person_id| self.memory_owned_by(&memory.memory_id, person_id))
        });
        LifecycleSummary {
            temporary_observations: observations
                .clone()
                .filter(|observation| observation.lifecycle.retention == Retention::Temporary)
                .count(),
            durable_memories: memories
                .clone()
                .filter(|memory| memory.lifecycle.retention == Retention::Durable)
                .count(),
            sensitive_items: observations
                .clone()
                .filter(|observation| sensitivity_rank(&observation.lifecycle.sensitivity) >= 2)
                .count()
                + memories
                    .clone()
                    .filter(|memory| sensitivity_rank(&memory.lifecycle.sensitivity) >= 2)
                    .count(),
            expired_items: observations
                .filter(|observation| is_expired(&observation.lifecycle))
                .count()
                + memories
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
        &input.context,
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
        context: redact_work_context(&input.context),
        captured_at: timestamp(),
    })
}

/// A retry is the same event only if what identifies its payload is unchanged.
fn is_replay(stored: &Observation, input: &ObservationInput) -> bool {
    stored.session_id == input.session_id
        && stored.content == redact_sensitive_content(&input.content)
        && stored.role == input.role
        && stored.source == input.source
}

fn changed_replay(event_id: &str) -> StoreError {
    StoreError::Conflict(format!(
        "event id already used with different content: {event_id}"
    ))
}

fn outside_sync_scope(event_id: &str) -> StoreError {
    StoreError::Conflict(format!(
        "event already exists outside sync scope: {event_id}"
    ))
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

// PostgreSQL reads cast this string with `::bigint`, so the bound is i64::MAX, not u64::MAX.
fn validate_timestamp(value: &str, field: &str) -> Result<(), StoreError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|seconds| i64::try_from(*seconds).is_ok())
        .map(|_| ())
        .ok_or_else(|| {
            StoreError::Validation(format!(
                "{field} must be a Unix timestamp in seconds between 0 and {}",
                i64::MAX
            ))
        })
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
    if ["authorization: bearer ", "api_key=", "token=", "password="]
        .iter()
        .any(|marker| lower.contains(marker))
        || find_token_prefix(&lower, "sk-", 0).is_some()
        || contains_private_key_header(content)
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

fn floor_sensitivity(
    supplied: Option<&Sensitivity>,
    content: &str,
    concepts: &[String],
    context: &WorkContext,
) -> Sensitivity {
    let mut detected = classify_sensitivity(content);
    for text in concepts
        .iter()
        .map(String::as_str)
        .chain(human_context_texts(context))
    {
        if text.is_empty() {
            continue;
        }
        detected = stricter_sensitivity(detected, classify_sensitivity(text));
    }
    match supplied {
        Some(supplied) if sensitivity_rank(supplied) >= sensitivity_rank(&detected) => {
            supplied.clone()
        }
        _ => detected,
    }
}

fn stricter_sensitivity(current: Sensitivity, next: Sensitivity) -> Sensitivity {
    if sensitivity_rank(&current) >= sensitivity_rank(&next) {
        current
    } else {
        next
    }
}

/// Raises `current` when the human-readable context lists detect a stricter
/// label. Repository, branch, worktree, and commit are not scanned.
pub fn sensitivity_including_context(current: &Sensitivity, context: &WorkContext) -> Sensitivity {
    floor_sensitivity(Some(current), "", &[], context)
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
    for session in &mut bundle.sessions {
        if session.session_id.trim().is_empty() {
            return Err(StoreError::Validation(
                "export session_id must not be empty".to_owned(),
            ));
        }
        session.context = redact_work_context(&session.context);
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
        observation.context = redact_work_context(&observation.context);
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
        redact_memory_metadata(memory);
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

/// Device use is recorded at most this often, so read-only calls do not churn state.
const LAST_SEEN_THROTTLE_SECS: u64 = 60;

fn touch_last_seen(device: &mut Device) {
    let now = now_seconds();
    let stale = device.last_seen_at.parse::<u64>().map_or(true, |seen| {
        now.saturating_sub(seen) >= LAST_SEEN_THROTTLE_SECS
    });
    if stale {
        device.last_seen_at = now.to_string();
    }
}

/// Every device token starts with this, so a caller can skip lookups for other credentials.
pub const DEVICE_TOKEN_PREFIX: &str = "cairn-device-";

fn device_token(_device_id: &str) -> String {
    random_token("cairn-device")
}

fn human_context_texts(context: &WorkContext) -> impl Iterator<Item = &str> {
    context
        .files
        .iter()
        .chain(context.symbols.iter())
        .chain(context.commands.iter())
        .chain(context.tests.iter())
        .chain(context.services.iter())
        .chain(context.handoffs.iter())
        .chain(context.runbooks.iter())
        .map(String::as_str)
}

fn redact_string_list(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| redact_sensitive_content(value))
        .collect()
}

/// Redacts the human-readable work-context lists. Repository, branch,
/// worktree, and commit stay, including a supported secret in those fields.
pub fn redact_work_context(context: &WorkContext) -> WorkContext {
    WorkContext {
        project_id: context.project_id.clone(),
        repository: context.repository.clone(),
        branch: context.branch.clone(),
        worktree: context.worktree.clone(),
        commit: context.commit.clone(),
        files: redact_string_list(&context.files),
        symbols: redact_string_list(&context.symbols),
        commands: redact_string_list(&context.commands),
        tests: redact_string_list(&context.tests),
        services: redact_string_list(&context.services),
        handoffs: redact_string_list(&context.handoffs),
        runbooks: redact_string_list(&context.runbooks),
    }
}

/// Redacts human-readable memory metadata on a new row. Ids, timestamps,
/// commits, and provenance links stay.
pub fn redact_memory_metadata(memory: &mut Memory) {
    memory.context = redact_work_context(&memory.context);
    memory.next_steps = redact_string_list(&memory.next_steps);
    memory.unresolved_work = redact_string_list(&memory.unresolved_work);
    memory.promotion_reason = memory
        .promotion_reason
        .as_deref()
        .map(redact_sensitive_content);
}

pub fn redact_sensitive_content(content: &str) -> String {
    let mut redacted = content.to_owned();
    for marker in ["authorization: bearer ", "api_key=", "token=", "password="] {
        redacted = redact_after_marker(&redacted, marker);
    }
    redacted = redact_prefixed_secret(&redacted, "sk-");
    redact_private_key_blocks(&redacted)
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

// A prefix counts only where a token starts, so ordinary words such as "disk-full" stay intact.
fn find_token_prefix(lower: &str, prefix: &str, from: usize) -> Option<usize> {
    let mut search_from = from;
    while let Some(relative) = lower[search_from..].find(prefix) {
        let start = search_from + relative;
        if !lower[..start]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric)
        {
            return Some(start);
        }
        search_from = start + prefix.len();
    }
    None
}

fn redact_prefixed_secret(content: &str, prefix: &str) -> String {
    let lower = content.to_ascii_lowercase();
    let prefix = prefix.to_ascii_lowercase();
    let mut output = String::with_capacity(content.len());
    let mut cursor = 0;
    while let Some(start) = find_token_prefix(&lower, &prefix, cursor) {
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

const PRIVATE_KEY_LABELS: &[&str] = &[
    "encrypted private key",
    "openssh private key",
    "rsa private key",
    "ec private key",
    "private key",
];

fn known_private_key_label(label: &str) -> Option<&'static str> {
    PRIVATE_KEY_LABELS
        .iter()
        .copied()
        .find(|candidate| *candidate == label)
}

fn private_key_header(line: &str) -> Option<(usize, &'static str)> {
    let lower = line.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(relative) = lower[search_from..].find("-----begin ") {
        let start = search_from + relative;
        let after = start + "-----begin ".len();
        let Some(end_rel) = lower[after..].find("-----") else {
            break;
        };
        let label_end = after + end_rel;
        if let Some(label) = known_private_key_label(&lower[after..label_end]) {
            return Some((start, label));
        }
        search_from = label_end + "-----".len();
    }
    None
}

fn contains_private_key_header(content: &str) -> bool {
    let mut cursor = 0;
    while cursor < content.len() {
        let next_newline = content[cursor..]
            .find('\n')
            .map(|offset| cursor + offset + 1)
            .unwrap_or(content.len());
        if private_key_header(&content[cursor..next_newline]).is_some() {
            return true;
        }
        cursor = next_newline;
    }
    false
}

fn is_pem_continuation_line(line: &str) -> bool {
    let trimmed = line.trim_end_matches(['\r', '\n']).trim();
    if trimmed.is_empty() {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("proc-type:") || lower.starts_with("dek-info:") {
        return true;
    }
    trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '='))
}

/// Resume offset after optional PEM base64 glued to a truncated
/// `-----END <label>` that has no closing `-----`.
fn truncated_footer_resume(line_lower: &str, label: &str) -> Option<usize> {
    let token = format!("-----end {label}");
    let bytes = line_lower.as_bytes();
    let mut index = 0;
    while index < bytes.len()
        && matches!(bytes[index], b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' | b'=')
    {
        index += 1;
    }
    let rest = line_lower.get(index..)?;
    if !rest.starts_with(&token) {
        return None;
    }
    let after = index + token.len();
    if line_lower
        .get(after..)
        .is_some_and(|suffix| suffix.starts_with("-----"))
    {
        return None;
    }
    Some(after)
}

/// A private-key header with no matching END drops that header line and
/// following base64 or OpenSSL metadata lines. Base64 glued to a truncated
/// `-----END <label>` drops that prefix and the footer token; text after the
/// token stays.
fn redact_private_key_blocks(content: &str) -> String {
    let mut output = String::with_capacity(content.len());
    let mut cursor = 0;
    while cursor < content.len() {
        let next_newline = content[cursor..]
            .find('\n')
            .map(|offset| cursor + offset + 1)
            .unwrap_or(content.len());
        let line = &content[cursor..next_newline];
        let Some((header_at, label)) = private_key_header(line) else {
            output.push_str(line);
            cursor = next_newline;
            continue;
        };
        output.push_str(&line[..header_at]);
        output.push_str(REDACTED);
        let end_marker = format!("-----end {label}-----");
        let line_lower = line.to_ascii_lowercase();
        if let Some(end_rel) = line_lower[header_at..].find(&end_marker) {
            cursor += header_at + end_rel + end_marker.len();
            continue;
        }
        if line.ends_with('\n') {
            output.push('\n');
        }
        cursor = next_newline;
        while cursor < content.len() {
            let body_end = content[cursor..]
                .find('\n')
                .map(|offset| cursor + offset + 1)
                .unwrap_or(content.len());
            let body = &content[cursor..body_end];
            let body_lower = body.to_ascii_lowercase();
            if let Some(end_rel) = body_lower.find(&end_marker) {
                cursor += end_rel + end_marker.len();
                break;
            }
            if let Some(resume) = truncated_footer_resume(&body_lower, label) {
                cursor += resume;
                break;
            }
            if !is_pem_continuation_line(body) {
                break;
            }
            cursor = body_end;
        }
    }
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

/// A summary is stale when a retained observation is not among its sources, or a source
/// has been forgotten (the summary is hidden until it is derived again without it).
fn summary_is_stale(
    summary: &Memory,
    observations: &[&Observation],
    forgotten_ids: &HashSet<String>,
) -> bool {
    observations
        .iter()
        .any(|observation| !summary.source_event_ids.contains(&observation.event_id))
        || summary
            .source_event_ids
            .iter()
            .any(|event_id| forgotten_ids.contains(event_id))
}

/// A re-derived summary keeps what the earlier one accumulated: verification, lifecycle (the
/// sensitivity can only rise) and creation time.
pub fn carry_summary_state(earlier: &Memory, derived: &mut Memory) {
    derived.lifecycle = Lifecycle {
        sensitivity: stricter_sensitivity(
            earlier.lifecycle.sensitivity.clone(),
            derived.lifecycle.sensitivity.clone(),
        ),
        ..earlier.lifecycle.clone()
    };
    derived.verified_commit.clone_from(&earlier.verified_commit);
    derived.created_at.clone_from(&earlier.created_at);
}

pub fn session_memory_id(person_id: Option<&str>, session_id: &str) -> String {
    match person_id.filter(|person_id| !person_id.is_empty()) {
        Some(person_id) => format!("memory-{person_id}-{session_id}"),
        None => format!("memory-{session_id}"),
    }
}

fn keeps_expired_source(memory: &Memory) -> bool {
    if memory.lifecycle.retention != Retention::Durable || is_expired(&memory.lifecycle) {
        return false;
    }
    matches!(
        memory.kind,
        MemoryKind::Explicit
            | MemoryKind::Lesson
            | MemoryKind::Correction
            | MemoryKind::Decision
            | MemoryKind::SessionSummary
    ) || memory.project_id.is_some()
        || memory.scope == MemoryScope::Project
}

pub fn derive_session_memory(
    person_id: Option<&str>,
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
    let mut memory = Memory {
        memory_id: session_memory_id(person_id, session_id),
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
    };
    redact_memory_metadata(&mut memory);
    Ok(memory)
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

fn decision_ids(memories: &[Memory]) -> HashSet<&str> {
    memories
        .iter()
        .filter(|memory| memory.kind == MemoryKind::Decision)
        .map(|memory| memory.memory_id.as_str())
        .collect()
}

/// Among results with equal scores, a decision (project decision) ranks before any other
/// memory. Scores are unchanged and the stable sort keeps the earlier order otherwise.
pub fn prefer_decision(results: &mut [RecallResult], memories: &[Memory]) {
    let decisions = decision_ids(memories);
    results.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                decisions
                    .contains(right.memory_id.as_str())
                    .cmp(&decisions.contains(left.memory_id.as_str()))
            })
    });
}

/// Among results with equal scores that list each other as contradictions, the newer
/// memory ranks first, unless that would put a non-decision before a decision. Nothing is
/// resolved or hidden. Adjacent swaps only, so unrelated order is unchanged and no
/// comparator has to be a total order.
pub fn prefer_newer_contradiction(results: &mut [RecallResult], memories: &[Memory]) {
    let created: HashMap<&str, &str> = memories
        .iter()
        .map(|memory| (memory.memory_id.as_str(), memory.created_at.as_str()))
        .collect();
    let decisions = decision_ids(memories);
    let older_first = |earlier: &RecallResult, later: &RecallResult| {
        earlier.score == later.score
            && decisions.contains(earlier.memory_id.as_str())
                == decisions.contains(later.memory_id.as_str())
            && (earlier.contradiction_ids.contains(&later.memory_id)
                || later.contradiction_ids.contains(&earlier.memory_id))
            && created.get(earlier.memory_id.as_str()) < created.get(later.memory_id.as_str())
    };
    for index in 1..results.len() {
        let mut at = index;
        while at > 0 && older_first(&results[at - 1], &results[at]) {
            results.swap(at - 1, at);
            at -= 1;
        }
    }
}

pub fn recall_result(
    memory: &Memory,
    score: f32,
    all_memories: &[Memory],
    forgotten_ids: &HashSet<String>,
    filter: &RecallFilter,
) -> RecallResult {
    // The slice has no ownership data, so every live successor counts. PostgreSQL passes
    // one person's rows, which is the same rule.
    let superseded = superseded_targets(all_memories, forgotten_ids, &HashMap::new());
    recall_result_with(
        memory,
        score,
        all_memories,
        forgotten_ids,
        filter,
        &superseded,
    )
}

fn recall_result_with(
    memory: &Memory,
    score: f32,
    all_memories: &[Memory],
    forgotten_ids: &HashSet<String>,
    filter: &RecallFilter,
    superseded_ids: &HashSet<&str>,
) -> RecallResult {
    let (stale, stale_reason) = stale_signal(memory, filter);
    let contradiction_ids = contradiction_ids(memory, all_memories, forgotten_ids, superseded_ids);
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
        promoted_from: live_origin(memory, all_memories, forgotten_ids, superseded_ids),
        promotion_reason: memory.promotion_reason.clone(),
        verified_commit: memory.verified_commit.clone(),
        score,
    }
}

/// The personal memory a promoted copy came from, only while that memory is still recalled.
/// Once it is forgotten, expired or superseded the link is inert: stored, never exposed.
fn live_origin(
    memory: &Memory,
    all_memories: &[Memory],
    forgotten_ids: &HashSet<String>,
    superseded_ids: &HashSet<&str>,
) -> Option<String> {
    let origin = memory.promoted_from.as_deref()?;
    all_memories
        .iter()
        .find(|candidate| {
            candidate.memory_id == origin
                && !memory_forgotten(candidate, forgotten_ids)
                && !is_expired(&candidate.lifecycle)
                && !superseded_ids.contains(origin)
        })
        .map(|candidate| candidate.memory_id.clone())
}

/// Why-changed priority of a memory: whether it carries a decision reason, then its
/// relationship count. These are the same helpers `recall_result` fills in, so a store can
/// rank before building results.
pub fn why_changed_priority(memory: &Memory) -> (bool, usize) {
    (
        extract_decision_reason(&memory.content).is_some(),
        code_relationships(memory).len(),
    )
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
    let superseded = superseded_targets(memories, &forgotten, &HashMap::new());
    let mut seen = HashSet::new();
    let mut conflicts = Vec::new();
    for memory in memories
        .iter()
        .filter(|memory| memory.scope == MemoryScope::Project)
    {
        for other_id in contradiction_ids(memory, memories, &forgotten, &superseded) {
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

fn memory_forgotten(memory: &Memory, forgotten_ids: &HashSet<String>) -> bool {
    forgotten_ids.contains(&memory.memory_id)
        || memory
            .source_event_ids
            .iter()
            .any(|event_id| forgotten_ids.contains(event_id))
}

fn live_superseded_target<'a>(
    candidate: &'a Memory,
    forgotten_ids: &HashSet<String>,
) -> Option<&'a str> {
    let target = candidate.supersedes.as_deref()?;
    (!memory_forgotten(candidate, forgotten_ids) && !is_expired(&candidate.lifecycle))
        .then_some(target)
}

/// Ids hidden by a live successor in `memories`. A link whose successor has an owner that
/// differs from the target's owner (or the target has none) is stored but does not count:
/// an account cannot hide another account's memory. A successor with no owner is local
/// or pre-account data and counts. Pass an empty map when the slice has no ownership.
fn superseded_targets<'a>(
    memories: &'a [Memory],
    forgotten_ids: &HashSet<String>,
    owners: &HashMap<&str, &str>,
) -> HashSet<&'a str> {
    memories
        .iter()
        .filter_map(|candidate| {
            let target = live_superseded_target(candidate, forgotten_ids)?;
            let foreign = owners
                .get(candidate.memory_id.as_str())
                .is_some_and(|successor| owners.get(target) != Some(successor));
            (!foreign).then_some(target)
        })
        .collect()
}

/// Promoted copies whose personal origin a live correction supersedes. The copy was made
/// from the old text, so it is hidden like the origin (ADR 0010).
fn stale_copies<'a>(
    memories: &'a [Memory],
    superseded: &HashSet<&str>,
) -> impl Iterator<Item = &'a str> {
    memories
        .iter()
        .filter(|memory| {
            memory
                .promoted_from
                .as_deref()
                .is_some_and(|origin| superseded.contains(origin))
        })
        .map(|memory| memory.memory_id.as_str())
}

fn contradiction_ids(
    memory: &Memory,
    all_memories: &[Memory],
    forgotten_ids: &HashSet<String>,
    superseded_ids: &HashSet<&str>,
) -> Vec<String> {
    let target_negative = contains_negation(&memory.content);
    let target_topics = topic_tokens(&memory.content);
    if target_topics.is_empty() {
        return Vec::new();
    }
    all_memories
        .iter()
        .filter(|other| other.memory_id != memory.memory_id)
        .filter(|other| {
            !memory_forgotten(other, forgotten_ids)
                && !is_expired(&other.lifecycle)
                && !superseded_ids.contains(other.memory_id.as_str())
        })
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
    let record_lifecycle = Lifecycle::explicit(&lifecycle, content, &concepts, &context);
    let stored_context = redact_work_context(&context);
    Ok(Memory {
        memory_id: format!("memory-explicit-{suffix}"),
        session_id: format!("session-explicit-{suffix}"),
        content: build_explicit_memory(content, concepts),
        source_event_ids: vec![event_id],
        context: stored_context,
        lifecycle: record_lifecycle,
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
            project_id: None,
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

    // CM-AUDIT-012: `supersedes` is stored provenance. It hides its target only when the
    // successor does not belong to a different person than the target.
    fn linked_memory(label: &str, supersedes: &str) -> Memory {
        let mut memory = build_explicit_memory_record(
            &format!("betalink {label} unrelated"),
            Vec::new(),
            WorkContext::default(),
            LifecycleInput::default(),
            MemoryKind::Explicit,
        )
        .expect("linked memory");
        memory.memory_id = format!("memory-link-{label}");
        memory.source_event_ids = vec![format!("event-link-{label}")];
        memory.supersedes = Some(supersedes.to_owned());
        memory
    }

    fn import_memories(
        store: &mut MemoryStore,
        token: &str,
        memories: Vec<Memory>,
    ) -> Result<(), StoreError> {
        let mut bundle = store.export_for_account(token)?;
        bundle.sessions.clear();
        bundle.observations.clear();
        bundle.forgotten_ids.clear();
        bundle.memories = memories;
        store.import_for_account(token, bundle)?;
        Ok(())
    }

    fn ids_of(results: &[RecallResult]) -> Vec<&str> {
        let mut ids = results
            .iter()
            .map(|result| result.memory_id.as_str())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    }

    fn remember_owned(
        store: &mut MemoryStore,
        token: &str,
        content: &str,
    ) -> Result<Memory, StoreError> {
        store.remember_for_account(
            token,
            content,
            Vec::new(),
            WorkContext::default(),
            LifecycleInput::default(),
            MemoryKind::Explicit,
        )
    }

    #[test]
    fn foreign_supersedes_link_is_stored_but_does_not_change_account_recall()
    -> Result<(), StoreError> {
        let path = temp_store_path("foreign-supersedes");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let other = store.register_account("other@example.test", "correct horse battery staple")?;
        let filter = RecallFilter::default();
        let kept = remember_owned(
            &mut store,
            &owner.token,
            "alphalink checkout cache redis fits",
        )?;
        let target = remember_owned(
            &mut store,
            &owner.token,
            "alphalink checkout cache avoid redis",
        )?;
        store.start_session_for_account(&owner.token, "link-session", context())?;
        store.capture_for_account(
            &owner.token,
            observation_for("link-session", "link-evt", "resumelink resumefact"),
        )?;
        let summary = store.end_session_for_account(&owner.token, "link-session")?;
        let mut expected = vec![kept.memory_id.as_str(), target.memory_id.as_str()];
        expected.sort_unstable();

        import_memories(
            &mut store,
            &other.token,
            vec![
                linked_memory("target", &target.memory_id),
                linked_memory("summary", &summary.memory_id),
            ],
        )?;

        let hits = store.search_for_account(&owner.token, "alphalink", 10, &filter)?;
        assert_eq!(ids_of(&hits), expected);
        assert!(
            hits.iter()
                .all(|hit| hit.contradictory && hit.contradiction_ids.len() == 1)
        );
        let pack = store.context_for_account(&owner.token, "alphalink", 10, &filter)?;
        assert_eq!(ids_of(&pack.results), expected);
        let resume = store.resume_for_account(&owner.token, 10, &filter)?;
        assert_eq!(ids_of(&resume.results), vec![summary.memory_id.as_str()]);

        // The other account gets only its own rows and no content from the owner.
        let own = store.context_for_account(&other.token, "betalink", 10, &filter)?;
        assert_eq!(own.results.len(), 2);
        assert!(
            store
                .search_for_account(&other.token, "alphalink", 10, &filter)?
                .is_empty()
        );
        assert!(!format!("{own:?}").contains("alphalink"));

        // Export keeps the target row and the original link on both sides.
        let owner_export = store.export_for_account(&owner.token)?;
        assert!(
            owner_export
                .memories
                .iter()
                .any(|m| m.memory_id == target.memory_id)
        );
        let other_export = store.export_for_account(&other.token)?;
        let link = other_export
            .memories
            .iter()
            .find(|m| m.memory_id == "memory-link-target")
            .expect("stored link row");
        assert_eq!(link.supersedes.as_deref(), Some(target.memory_id.as_str()));
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn same_owner_and_unowned_successors_still_hide_their_target() -> Result<(), StoreError> {
        let path = temp_store_path("owned-supersedes");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let other = store.register_account("other@example.test", "correct horse battery staple")?;
        let filter = RecallFilter::default();

        // Same owner: the correction hides the target, and forgetting it revives the target.
        let old = remember_owned(&mut store, &owner.token, "samelink original claim")?;
        let fix = store.correct_for_account(
            &owner.token,
            &old.memory_id,
            "samelink corrected claim",
            Vec::new(),
            WorkContext::default(),
            LifecycleInput::default(),
        )?;
        let hits = store.search_for_account(&owner.token, "samelink", 10, &filter)?;
        assert_eq!(ids_of(&hits), vec![fix.memory_id.as_str()]);
        store.forget_for_account(&owner.token, &fix.memory_id)?;
        let hits = store.search_for_account(&owner.token, "samelink", 10, &filter)?;
        assert_eq!(ids_of(&hits), vec![old.memory_id.as_str()]);

        // Unowned local successor of an owned target keeps hiding it (CLI and MCP).
        let owned = remember_owned(&mut store, &owner.token, "locallink owned claim")?;
        let local_fix = store.correct(
            &owned.memory_id,
            "locallink local correction",
            Vec::new(),
            WorkContext::default(),
            LifecycleInput::default(),
        )?;
        let hits = store.search_with_filter("locallink", 10, &filter)?;
        assert_eq!(ids_of(&hits), vec![local_fix.memory_id.as_str()]);

        // An owned successor of an unowned target is foreign: the link stays inert.
        let legacy = store.remember(
            "legacylink unowned claim",
            Vec::new(),
            WorkContext::default(),
        )?;
        import_memories(
            &mut store,
            &other.token,
            vec![linked_memory("legacy", &legacy.memory_id)],
        )?;
        let hits = store.search_with_filter("legacylink", 10, &filter)?;
        assert_eq!(ids_of(&hits), vec![legacy.memory_id.as_str()]);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn foreign_link_to_unknown_target_stays_inert_after_the_target_is_restored()
    -> Result<(), StoreError> {
        let path = temp_store_path("late-supersedes");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let other = store.register_account("other@example.test", "correct horse battery staple")?;
        let filter = RecallFilter::default();
        import_memories(
            &mut store,
            &other.token,
            vec![linked_memory("late", "memory-later-target")],
        )?;
        assert!(
            store
                .export_for_account(&other.token)?
                .memories
                .iter()
                .any(|m| {
                    m.memory_id == "memory-link-late"
                        && m.supersedes.as_deref() == Some("memory-later-target")
                })
        );

        let mut restored = linked_memory("restored", "memory-unrelated");
        restored.memory_id = "memory-later-target".to_owned();
        restored.supersedes = None;
        restored.content = "restoredlink owner backup".to_owned();
        import_memories(&mut store, &owner.token, vec![restored])?;
        let hits = store.search_for_account(&owner.token, "restoredlink", 10, &filter)?;
        assert_eq!(ids_of(&hits), vec!["memory-later-target"]);
        fs::remove_file(path)?;
        Ok(())
    }

    // CM-AUDIT-013: an account import applies a tombstone only for ids the caller owns or
    // brings as rows. A foreign-owned id rejects the import; other ids are ignored.
    fn import_forgotten(
        store: &mut MemoryStore,
        token: &str,
        memories: Vec<Memory>,
        forgotten: &[&str],
    ) -> Result<ImportResult, StoreError> {
        let mut bundle = store.export_for_account(token)?;
        bundle.sessions.clear();
        bundle.observations.clear();
        bundle.memories = memories;
        bundle.forgotten_ids = forgotten.iter().map(|id| (*id).to_owned()).collect();
        store.import_for_account(token, bundle)
    }

    #[test]
    fn foreign_tombstones_reject_the_whole_import_and_change_nothing() -> Result<(), StoreError> {
        let path = temp_store_path("foreign-tombstone");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let other = store.register_account("other@example.test", "correct horse battery staple")?;
        let filter = RecallFilter::default();
        let kept = remember_owned(&mut store, &owner.token, "tombfact alpha note")?;
        store.start_session_for_account(&owner.token, "tomb-session", context())?;
        store.capture_for_account(
            &owner.token,
            observation_for("tomb-session", "tomb-evt", "tombsummary fact"),
        )?;
        let summary = store.end_session_for_account(&owner.token, "tomb-session")?;
        let before = fs::read(&path)?;

        // A memory id, then an observation event id; each comes with a valid new row.
        for foreign in [kept.memory_id.as_str(), "tomb-evt"] {
            let error = import_forgotten(
                &mut store,
                &other.token,
                vec![linked_memory("tomb-new", "memory-unrelated")],
                &[foreign],
            )
            .expect_err("foreign tombstone must reject the import");
            assert!(
                matches!(&error, StoreError::Validation(text) if text == "account does not own the memory")
            );
            assert_eq!(fs::read(&path)?, before, "nothing is written on rejection");
        }

        let reopened = MemoryStore::open(&path)?;
        let hits = reopened.search_for_account(&owner.token, "tombfact", 10, &filter)?;
        assert_eq!(ids_of(&hits), vec![kept.memory_id.as_str()]);
        let resume = reopened.resume_for_account(&owner.token, 10, &filter)?;
        assert_eq!(ids_of(&resume.results), vec![summary.memory_id.as_str()]);
        assert!(
            reopened
                .export_for_account(&other.token)?
                .memories
                .is_empty()
        );
        assert!(reopened.state.forgotten_ids.is_empty());
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn unknown_tombstones_are_ignored_and_source_ids_are_not_permission() -> Result<(), StoreError>
    {
        let path = temp_store_path("unknown-tombstone");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let other = store.register_account("other@example.test", "correct horse battery staple")?;

        // An id nobody owns, and a source id named by the importer's own new row.
        let mut row = linked_memory("tomb-src", "memory-unrelated");
        row.source_event_ids = vec!["tomb-future-evt".to_owned()];
        let result = import_forgotten(
            &mut store,
            &other.token,
            vec![row],
            &["tomb-unknown", "tomb-future-evt", "memory-link-tomb-src"],
        )?;
        assert_eq!(result.imported_memories, 1);
        assert_eq!(
            result.imported_forgotten_ids, 1,
            "only the row's own id counts"
        );
        let expected = HashSet::from(["memory-link-tomb-src".to_owned()]);
        assert_eq!(store.state.forgotten_ids, expected);

        // The owner's later capture of that event id is not poisoned.
        store.start_session_for_account(&owner.token, "tomb-later", context())?;
        store.capture_for_account(
            &owner.token,
            observation_for("tomb-later", "tomb-future-evt", "latertomb fact"),
        )?;
        let summary = store.end_session_for_account(&owner.token, "tomb-later")?;
        assert!(summary.content.contains("latertomb"));
        assert_eq!(MemoryStore::open(&path)?.state.forgotten_ids, expected);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn own_tombstones_apply_and_forgotten_memories_survive_a_restore() -> Result<(), StoreError> {
        let path = temp_store_path("own-tombstone");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let filter = RecallFilter::default();
        let existing = remember_owned(&mut store, &owner.token, "owntomb existing note")?;
        let result = import_forgotten(
            &mut store,
            &owner.token,
            vec![linked_memory("own-new", "memory-unrelated")],
            &[&existing.memory_id, "memory-link-own-new"],
        )?;
        assert_eq!(result.imported_forgotten_ids, 2);
        assert!(
            store
                .search_for_account(&owner.token, "owntomb", 10, &filter)?
                .is_empty()
        );
        fs::remove_file(&path)?;

        // A normal export of a forgotten memory restores into a new store and account.
        let source_path = temp_store_path("restore-source");
        let mut source = MemoryStore::open(&source_path)?;
        let account =
            source.bootstrap_account("one@example.test", "correct horse battery staple")?;
        let keep = remember_owned(&mut source, &account.token, "restoretomb keep")?;
        let drop = remember_owned(&mut source, &account.token, "restoretomb drop")?;
        source.forget_for_account(&account.token, &drop.memory_id)?;
        let bundle = source.export_for_account(&account.token)?;
        assert_eq!(
            bundle.forgotten_ids.len(),
            2,
            "the memory and its source event id"
        );

        let target_path = temp_store_path("restore-target");
        let mut target = MemoryStore::open(&target_path)?;
        let restored =
            target.bootstrap_account("two@example.test", "correct horse battery staple")?;
        let result = target.import_for_account(&restored.token, bundle)?;
        assert_eq!(result.imported_memories, 2);
        assert_eq!(
            result.imported_forgotten_ids, 1,
            "the source id has no row and is ignored"
        );
        let hits = target.search_for_account(&restored.token, "restoretomb", 10, &filter)?;
        assert_eq!(ids_of(&hits), vec![keep.memory_id.as_str()]);
        fs::remove_file(source_path)?;
        fs::remove_file(target_path)?;
        Ok(())
    }

    // CM-AUDIT-013 B1: memory ids and event ids share one id space. A row another account
    // owns under an id must make capture and sync push of that id fail loudly.
    #[test]
    fn a_foreign_memory_row_id_cannot_poison_a_later_capture_or_sync_push() -> Result<(), StoreError>
    {
        let path = temp_store_path("tombstone-namespace");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let other = store.register_account("other@example.test", "correct horse battery staple")?;
        let device = store.register_device("dev-ns", &owner.account.person_id, None, "Owner")?;
        store.start_session_for_account(&owner.token, "ns-session", context())?;

        // The importer brings a memory row named like the owner's future event id, with a tombstone.
        let mut squat = linked_memory("squat", "memory-unrelated");
        squat.memory_id = "evt-future-1".to_owned();
        squat.supersedes = None;
        let imported = import_forgotten(&mut store, &other.token, vec![squat], &["evt-future-1"])?;
        assert_eq!(imported.imported_forgotten_ids, 1);

        let events_before = store.state.sync_events.len();
        let cursor_before = store.state.next_sync_cursor;
        let input = observation_for("ns-session", "evt-future-1", "nsprivate owner fact");
        let capture = store
            .capture_for_account(&owner.token, input.clone())
            .expect_err("a foreign memory row owns the id");
        assert!(
            matches!(&capture, StoreError::Validation(text) if text == "event belongs to another person: evt-future-1")
        );
        let push = store
            .sync_push(&device.token, input.clone())
            .expect_err("the device push is rejected the same way");
        assert!(
            matches!(&push, StoreError::Validation(text) if text == "event belongs to another person: evt-future-1")
        );
        assert!(store.observations().all(|o| o.event_id != "evt-future-1"));
        assert_eq!(store.state.sync_events.len(), events_before);
        assert_eq!(store.state.next_sync_cursor, cursor_before);
        assert!(!format!("{capture}{push}").contains("nsprivate"));
        // Only the device's last-seen time may change, and memory matches the disk.
        let reopened = MemoryStore::open(&path)?;
        assert_eq!(
            reopened.state.devices[0].device.last_seen_at,
            store.state.devices[0].device.last_seen_at
        );
        assert_eq!(
            reopened.state.observation_owners.len(),
            store.state.observation_owners.len()
        );
        assert_eq!(reopened.state.sync_events.len(), events_before);
        let mut reopened = reopened;
        assert!(
            reopened
                .capture_for_account(&owner.token, input.clone())
                .is_err()
        );

        // A bad token fails on authentication, before the id is looked at.
        let bad = store
            .capture_for_account("not-a-token", input.clone())
            .expect_err("bad token");
        assert!(!bad.to_string().contains("belongs to another person"));
        let bad = store
            .sync_push("not-a-token", input)
            .expect_err("bad device token");
        assert!(!bad.to_string().contains("belongs to another person"));

        // Unaffected: a normal capture, device adoption of it, and a memory row the same
        // person owns under an id it later captures.
        store.capture_for_account(
            &owner.token,
            observation_for("ns-session", "evt-ok", "nsok fact"),
        )?;
        let adopted = store.sync_push(
            &device.token,
            observation_for("ns-session", "evt-ok", "nsok fact"),
        )?;
        assert!(adopted.accepted && !adopted.duplicate);
        let mut own = linked_memory("own", "memory-unrelated");
        own.memory_id = "evt-own-1".to_owned();
        own.supersedes = None;
        import_forgotten(&mut store, &owner.token, vec![own], &[])?;
        store.capture_for_account(
            &owner.token,
            observation_for("ns-session", "evt-own-1", "nsown fact"),
        )?;
        // An observation another person already owns cannot be claimed as a memory row.
        let mut claim = linked_memory("claim", "memory-unrelated");
        claim.memory_id = "evt-ok".to_owned();
        claim.supersedes = None;
        assert!(import_forgotten(&mut store, &other.token, vec![claim], &[]).is_err());
        fs::remove_file(path)?;
        Ok(())
    }

    // CM-AUDIT-014 finding 1: a project-linked row in an account import must refer to a
    // project that account owns, and a stored foreign row stays out of the owner's views.
    fn project_row(id: &str, project_id: &str, content: &str) -> Memory {
        let mut memory = build_explicit_memory_record(
            content,
            Vec::new(),
            context(),
            LifecycleInput::default(),
            MemoryKind::Explicit,
        )
        .expect("project row");
        memory.memory_id = id.to_owned();
        memory.source_event_ids = vec![format!("event-{id}")];
        memory.scope = MemoryScope::Project;
        memory.project_id = Some(project_id.to_owned());
        memory
    }

    fn owned_project(
        store: &mut MemoryStore,
        token: &str,
        content: &str,
    ) -> Result<(String, Memory), StoreError> {
        let project = store.register_project(token, "cairn-demo", "Demo", None)?;
        let source = store.remember_for_account(
            token,
            content,
            Vec::new(),
            context(),
            LifecycleInput::default(),
            MemoryKind::Explicit,
        )?;
        let promoted =
            store.promote_memory(token, &project.project_id, &source.memory_id, "why")?;
        Ok((project.project_id, promoted))
    }

    fn remember_with_context(
        store: &mut MemoryStore,
        token: &str,
        content: &str,
    ) -> Result<Memory, StoreError> {
        store.remember_for_account(
            token,
            content,
            Vec::new(),
            context(),
            LifecycleInput::default(),
            MemoryKind::Explicit,
        )
    }

    #[test]
    fn account_import_rejects_project_rows_the_caller_does_not_own() -> Result<(), StoreError> {
        let path = temp_store_path("project-import");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let outsider =
            store.register_account("outsider@example.test", "correct horse battery staple")?;
        let member =
            store.register_account("member@example.test", "correct horse battery staple")?;
        let (project_id, _) = owned_project(&mut store, &owner.token, "pimport owner decision")?;
        join_project(&mut store, &member.token, &project_id)?;
        let before = fs::read(&path)?;
        let owners_before = store.state.memory_owners.len();

        // Outsider, invited member and a missing project each reject the whole bundle:
        // the valid row and its tombstone are not stored either.
        for (token, project) in [
            (&outsider.token, project_id.as_str()),
            (&member.token, project_id.as_str()),
            (&outsider.token, "project-missing"),
        ] {
            let mut fine = linked_memory("pi-fine", "memory-unrelated");
            fine.supersedes = None;
            let bad = project_row("memory-pi-bad", project, "pibad claim");
            let error =
                import_forgotten(&mut store, token, vec![fine, bad], &["memory-link-pi-fine"])
                    .expect_err("a foreign or missing project rejects the import");
            assert!(
                matches!(&error, StoreError::Validation(text) if text == "account does not own the project")
            );
            assert_eq!(fs::read(&path)?, before, "nothing is written on rejection");
        }
        assert_eq!(store.state.memory_owners.len(), owners_before);
        assert!(store.state.forgotten_ids.is_empty());
        let reopened = MemoryStore::open(&path)?;
        assert!(
            reopened
                .export_for_account(&outsider.token)?
                .memories
                .is_empty()
        );
        assert!(
            reopened
                .export_for_account(&member.token)?
                .memories
                .is_empty()
        );

        // Authentication comes before the project lookup.
        let mut bundle = store.export_for_account(&outsider.token)?;
        bundle.memories = vec![project_row("memory-pi-auth", &project_id, "piauth claim")];
        let error = store
            .import_for_account("not-a-token", bundle)
            .expect_err("bad token");
        assert!(error.to_string().contains("account authentication failed"));

        // A project with no owner record is not the caller's until it is claimed.
        store
            .state
            .project_owners
            .retain(|record| record.project_id != project_id);
        let legacy = project_row("memory-pi-legacy", &project_id, "pilegacy claim");
        assert!(import_forgotten(&mut store, &owner.token, vec![legacy.clone()], &[]).is_err());
        store.register_project(&owner.token, "cairn-demo", "Demo", None)?;

        // Own project: normal import.
        let result = import_forgotten(&mut store, &owner.token, vec![legacy], &[])?;
        assert_eq!(result.imported_memories, 1);
        let listed = store.project_memories(&owner.token, &project_id)?;
        assert!(listed.iter().any(|m| m.memory_id == "memory-pi-legacy"));
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn stored_foreign_project_rows_stay_out_of_owner_project_views() -> Result<(), StoreError> {
        let path = temp_store_path("project-foreign-rows");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let outsider =
            store.register_account("outsider@example.test", "correct horse battery staple")?;
        let member =
            store.register_account("member@example.test", "correct horse battery staple")?;
        let (project_id, mine) = owned_project(
            &mut store,
            &owner.token,
            "payments retry queue uses redis storage",
        )?;
        join_project(&mut store, &member.token, &project_id)?;
        store.share_project_memory(
            &owner.token,
            &project_id,
            &mine.memory_id,
            "member@example.test",
        )?;

        // Rows stored before the import rule: foreign-owned, and one local/unowned row.
        let note = project_row("memory-foreign-note", &project_id, "foreignnote claim");
        let mut hide = project_row("memory-foreign-hide", &project_id, "foreign replacement");
        hide.supersedes = Some(mine.memory_id.clone());
        let clash = project_row(
            "memory-foreign-clash",
            &project_id,
            "payments retry queue never uses redis storage",
        );
        let local = project_row("memory-local-note", &project_id, "localnote claim");
        for row in [&note, &hide, &clash] {
            store.state.memory_owners.push(MemoryOwnerRecord {
                memory_id: row.memory_id.clone(),
                person_id: outsider.account.person_id.clone(),
            });
        }
        store.state.memories.extend([note, hide, clash, local]);
        store.persist()?;

        let expected = {
            let mut ids = vec![mine.memory_id.clone(), "memory-local-note".to_owned()];
            ids.sort();
            ids
        };
        let check = |store: &MemoryStore| -> Result<(), StoreError> {
            let mut listed = store
                .project_memories(&owner.token, &project_id)?
                .into_iter()
                .map(|m| m.memory_id)
                .collect::<Vec<_>>();
            listed.sort();
            assert_eq!(listed, expected, "owner list: own and local rows only");
            let candidates = store
                .share_candidates(&owner.token, &project_id, "member@example.test")?
                .into_iter()
                .map(|c| c.memory_id)
                .collect::<Vec<_>>();
            assert_eq!(candidates, vec!["memory-local-note".to_owned()]);
            assert!(
                store
                    .project_conflicts(&owner.token, &project_id)?
                    .is_empty()
            );
            let report = store.project_maintenance(&owner.token, &project_id, None)?;
            assert!(report.superseded_memory_ids.is_empty());
            let mut active = report.active_memory_ids;
            active.sort();
            assert_eq!(active, expected);
            // Controls: recall and the team view do not change.
            let hits =
                store.search_for_account(&owner.token, "payments", 10, &RecallFilter::default())?;
            assert!(ids_of(&hits).contains(&mine.memory_id.as_str()));
            assert!(
                store
                    .search_for_account(&owner.token, "foreignnote", 10, &RecallFilter::default())?
                    .is_empty()
            );
            let shared = store.shared_project_memories(&member.token, &project_id)?;
            assert_eq!(shared.len(), 1);
            assert_eq!(shared[0].memory.memory_id, mine.memory_id);
            Ok(())
        };
        check(&store)?;
        check(&MemoryStore::open(&path)?)?;

        // Nothing was deleted or rewritten: the outsider still exports its three rows.
        assert_eq!(store.export_for_account(&outsider.token)?.memories.len(), 3);
        fs::remove_file(path)?;
        Ok(())
    }

    // CM-AUDIT-014 B1/S1: promotion and direct share must not use a stored foreign row.
    #[test]
    fn stored_foreign_project_rows_do_not_block_promotion_or_reach_a_share()
    -> Result<(), StoreError> {
        let path = temp_store_path("project-foreign-promote");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let outsider =
            store.register_account("outsider@example.test", "correct horse battery staple")?;
        let member =
            store.register_account("member@example.test", "correct horse battery staple")?;
        let (project_id, _) = owned_project(&mut store, &owner.token, "promo first decision")?;
        join_project(&mut store, &member.token, &project_id)?;
        let source = remember_with_context(&mut store, &owner.token, "promo second source")?;
        let legacy_source = remember_with_context(&mut store, &owner.token, "promo legacy source")?;

        // Stored before the import rule: foreign rows, one naming the owner's unpromoted
        // source, and an unowned local row naming another source.
        let mut foreign = project_row("memory-foreign-promo", &project_id, "foreignpromo text");
        foreign.promoted_from = Some(source.memory_id.clone());
        let plain = project_row("memory-foreign-plain", &project_id, "foreignplain text");
        let mut local = project_row("memory-local-promo", &project_id, "localpromo text");
        local.promoted_from = Some(legacy_source.memory_id.clone());
        for row in [&foreign, &plain] {
            store.state.memory_owners.push(MemoryOwnerRecord {
                memory_id: row.memory_id.clone(),
                person_id: outsider.account.person_id.clone(),
            });
        }
        store.state.memories.extend([foreign, plain, local]);
        store.persist()?;
        let shares_before = store.state.project_shares.len();

        // The real promotion is created for the owner, not the foreign row, and repeats.
        let promoted = store.confirm_memory(
            &owner.token,
            &project_id,
            &source.memory_id,
            Some("abc123"),
            "why",
        )?;
        assert_ne!(promoted.memory_id, "memory-foreign-promo");
        assert_eq!(
            promoted.promoted_from.as_deref(),
            Some(source.memory_id.as_str())
        );
        assert!(store.memory_owned_by(&promoted.memory_id, &owner.account.person_id));
        let again = store.promote_memory(&owner.token, &project_id, &source.memory_id, "why")?;
        assert_eq!(again.memory_id, promoted.memory_id);
        let listed = store.project_memories(&owner.token, &project_id)?;
        assert!(listed.iter().any(|m| m.memory_id == promoted.memory_id));
        assert!(
            listed
                .iter()
                .all(|m| !m.memory_id.starts_with("memory-foreign"))
        );

        // An unowned local row is the owner's idempotent promotion, as before.
        let legacy =
            store.promote_memory(&owner.token, &project_id, &legacy_source.memory_id, "why")?;
        assert_eq!(legacy.memory_id, "memory-local-promo");

        // A direct share of a foreign id is rejected; verify already was. Own share works.
        for id in ["memory-foreign-plain", "memory-foreign-promo"] {
            let error = store
                .share_project_memory(&owner.token, &project_id, id, "member@example.test")
                .expect_err("a foreign row is not shareable");
            assert!(
                matches!(&error, StoreError::Validation(text) if text == &format!("unknown memory or event: {id}"))
            );
            assert!(
                store
                    .verify_project_memory(&owner.token, &project_id, id, "abc123")
                    .is_err()
            );
        }
        assert_eq!(store.state.project_shares.len(), shares_before);
        store.share_project_memory(
            &owner.token,
            &project_id,
            &promoted.memory_id,
            "member@example.test",
        )?;
        let shared = store.shared_project_memories(&member.token, &project_id)?;
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].memory.memory_id, promoted.memory_id);

        // Nothing was deleted: the outsider still exports both rows.
        assert_eq!(store.export_for_account(&outsider.token)?.memories.len(), 2);
        fs::remove_file(path)?;
        Ok(())
    }

    // CM-AUDIT-013 findings 2 and 3: an account forgets only ids it owns, and a forget
    // cascades only the source ids that account owns.
    fn assert_unknown_id(error: &StoreError, id: &str) {
        assert!(
            matches!(error, StoreError::Validation(text) if text == &format!("unknown memory or event: {id}")),
            "unexpected error {error}"
        );
    }

    #[test]
    fn account_forget_and_delete_require_an_owned_id() -> Result<(), StoreError> {
        let path = temp_store_path("forget-owned");
        let mut store = MemoryStore::open(&path)?;
        // Legacy rows written before accounts: an event, its summary and an explicit memory.
        store.start_session("legacy-sess", context())?;
        store.capture_observation(observation_for(
            "legacy-sess",
            "legacy-evt",
            "legacyobs fact",
        ))?;
        let summary = store.end_session("legacy-sess")?;
        let explicit = store.remember("legacymem note", Vec::new(), context())?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let other = store.register_account("other@example.test", "correct horse battery staple")?;
        let mine = remember_owned(&mut store, &owner.token, "forgetmine note")?;
        let before = fs::read(&path)?;

        // Unowned legacy, foreign and unknown ids all fail alike and write nothing.
        for id in [
            "legacy-evt",
            summary.memory_id.as_str(),
            explicit.memory_id.as_str(),
            mine.memory_id.as_str(),
            "no-such-id",
        ] {
            assert_unknown_id(&store.forget_for_account(&other.token, id).unwrap_err(), id);
            assert_unknown_id(&store.delete_for_account(&other.token, id).unwrap_err(), id);
        }
        assert_eq!(fs::read(&path)?, before, "nothing is written on rejection");
        assert!(store.state.forgotten_ids.is_empty());
        assert!(store.state.sync_deletions.is_empty());
        assert!(store.state.audit_events.is_empty());
        assert!(
            store
                .forget_for_account("not-a-token", "legacy-evt")
                .is_err()
        );

        // The trusted unscoped call still forgets legacy data; the owner forgets and deletes its own.
        store.forget("legacy-evt")?;
        assert!(store.state.forgotten_ids.contains("legacy-evt"));
        store.delete_for_account(&owner.token, &mine.memory_id)?;
        assert!(store.state.forgotten_ids.contains(&mine.memory_id));
        fs::remove_file(path)?;
        Ok(())
    }

    // Project actions never delete personal memory: the promoted copy keeps no source events,
    // so forget and retained delete of the copy stop at the copy and its shares.
    #[test]
    fn project_forget_and_delete_never_touch_personal_memory() -> Result<(), StoreError> {
        let path = temp_store_path("project-never-deletes");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let member =
            store.register_account("member@example.test", "correct horse battery staple")?;
        let project = store.register_project(&owner.token, "cairn-demo", "Demo", None)?;
        let project_id = project.project_id;
        join_project(&mut store, &member.token, &project_id)?;
        store.create_project_team(&owner.token, &project_id, "Core")?;
        let filter = RecallFilter::default();
        let promote_shared = |store: &mut MemoryStore, content: &str| {
            let source = remember_with_context(store, &owner.token, content)?;
            let copy = store.promote_memory(&owner.token, &project_id, &source.memory_id, "why")?;
            store.share_project_memory(
                &owner.token,
                &project_id,
                &copy.memory_id,
                "member@example.test",
            )?;
            Ok::<_, StoreError>((source, copy))
        };

        // Forget and retained delete of the copy, by the project and team owner.
        for (word, delete) in [("forgetcopy", false), ("deletecopy", true)] {
            let (source, copy) = promote_shared(&mut store, &format!("{word} personal fact"))?;
            assert!(copy.source_event_ids.is_empty());
            assert_eq!(
                copy.promoted_from.as_deref(),
                Some(source.memory_id.as_str())
            );
            let hits = store.search_for_account(&owner.token, word, 10, &filter)?;
            let origin = hits.iter().find(|hit| hit.memory_id == copy.memory_id);
            assert_eq!(
                origin.and_then(|hit| hit.promoted_from.as_deref()),
                Some(source.memory_id.as_str()),
                "the owner sees the origin"
            );
            if delete {
                store.delete_for_account(&owner.token, &copy.memory_id)?;
            } else {
                store.forget_for_account(&owner.token, &copy.memory_id)?;
            }
            let hits = store.search_for_account(&owner.token, word, 10, &filter)?;
            assert_eq!(ids_of(&hits), vec![source.memory_id.as_str()]);
            assert!(!store.state.forgotten_ids.contains(&source.memory_id));
            assert!(
                !store
                    .state
                    .forgotten_ids
                    .contains(&source.source_event_ids[0])
            );
            assert!(
                store
                    .project_memories(&owner.token, &project_id)?
                    .is_empty()
            );
            assert!(
                store
                    .shared_project_memories(&member.token, &project_id)?
                    .is_empty()
            );
        }

        // Forgetting the personal memory leaves the shared copy; the origin is never exposed.
        let (source, copy) = promote_shared(&mut store, "keepcopy personal fact")?;
        let shared = store.shared_project_memories(&member.token, &project_id)?;
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].memory.promoted_from, None);
        assert!(shared[0].memory.source_event_ids.is_empty());
        store.forget_for_account(&owner.token, &source.memory_id)?;
        let hits = store.search_for_account(&owner.token, "keepcopy", 10, &filter)?;
        assert_eq!(ids_of(&hits), vec![copy.memory_id.as_str()]);
        assert_eq!(hits[0].promoted_from, None);
        let listed = store.project_memories(&owner.token, &project_id)?;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].promoted_from, None);
        let shared = store.shared_project_memories(&member.token, &project_id)?;
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].memory.memory_id, copy.memory_id);
        assert_eq!(shared[0].memory.promoted_from, None);
        fs::remove_file(path)?;
        Ok(())
    }

    // A correction of a personal memory hides that memory's promoted copies; promoting the
    // correction links the new copy to the old copy, never to the personal id.
    #[test]
    fn personal_correction_supersedes_the_promoted_copy() -> Result<(), StoreError> {
        let path = temp_store_path("correction-copy");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let member =
            store.register_account("member@example.test", "correct horse battery staple")?;
        let project = store.register_project(&owner.token, "cairn-demo", "Demo", None)?;
        let project_id = project.project_id;
        join_project(&mut store, &member.token, &project_id)?;
        let source = remember_with_context(&mut store, &owner.token, "stalecopy old fact")?;
        let copy = store.promote_memory(&owner.token, &project_id, &source.memory_id, "why")?;
        store.share_project_memory(
            &owner.token,
            &project_id,
            &copy.memory_id,
            "member@example.test",
        )?;
        let correction = store.correct_for_account(
            &owner.token,
            &source.memory_id,
            "stalecopy new fact",
            Vec::new(),
            context(),
            LifecycleInput::default(),
        )?;
        let filter = RecallFilter::default();
        assert!(
            store
                .project_memories(&owner.token, &project_id)?
                .is_empty()
        );
        assert!(
            store
                .shared_project_memories(&member.token, &project_id)?
                .is_empty()
        );
        let hits = store.search_for_account(&owner.token, "stalecopy", 10, &filter)?;
        assert_eq!(ids_of(&hits), vec![correction.memory_id.as_str()]);
        let report = store.project_maintenance(&owner.token, &project_id, None)?;
        assert_eq!(report.superseded_memory_ids, vec![copy.memory_id.clone()]);

        let next = store.promote_memory(&owner.token, &project_id, &correction.memory_id, "why")?;
        assert_eq!(next.supersedes.as_deref(), Some(copy.memory_id.as_str()));
        let listed = store.project_memories(&owner.token, &project_id)?;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].memory_id, next.memory_id);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn forget_cascades_only_sources_the_caller_owns() -> Result<(), StoreError> {
        let dir = temp_store_dir("forget-cascade");
        let path = dir.join("store.json");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let other = store.register_account("other@example.test", "correct horse battery staple")?;
        let device =
            store.register_device("dev-cascade", &owner.account.person_id, None, "Owner")?;
        let filter = RecallFilter::default();
        store.start_session_for_account(&owner.token, "victim-sess", context())?;
        store.capture_for_account(
            &owner.token,
            observation_for("victim-sess", "victim-evt", "victimcascade fact"),
        )?;
        store.sync_push(
            &device.token,
            observation_for("victim-sess", "victim-evt", "victimcascade fact"),
        )?;
        let summary = store.end_session_for_account(&owner.token, "victim-sess")?;
        let note = remember_owned(&mut store, &owner.token, "victimnote explicit")?;

        // Finding 3: the importer's own row names a foreign event, a foreign memory and a
        // future event as sources. Forgetting that row must not tombstone any of them.
        let mut row = linked_memory("cascade", "memory-unrelated");
        row.supersedes = None;
        row.source_event_ids = vec![
            "victim-evt".to_owned(),
            note.memory_id.clone(),
            "future-evt".to_owned(),
        ];
        import_forgotten(&mut store, &other.token, vec![row], &[])?;
        store.forget_for_account(&other.token, "memory-link-cascade")?;
        let expected = HashSet::from(["memory-link-cascade".to_owned()]);
        assert_eq!(store.state.forgotten_ids, expected);
        assert!(
            store
                .state
                .sync_deletions
                .iter()
                .all(|deletion| deletion.item_id == "memory-link-cascade")
        );
        let hits = store.search_for_account(&owner.token, "victimnote", 10, &filter)?;
        assert_eq!(ids_of(&hits), vec![note.memory_id.as_str()]);
        let resume = store.resume_for_account(&owner.token, 10, &filter)?;
        assert!(ids_of(&resume.results).contains(&summary.memory_id.as_str()));
        assert_eq!(store.sync_pull(&device.token, 0)?.events.len(), 1);
        store.start_session_for_account(&owner.token, "later-sess", context())?;
        store.capture_for_account(
            &owner.token,
            observation_for("later-sess", "future-evt", "latercascade fact"),
        )?;
        assert!(
            store
                .end_session_for_account(&owner.token, "later-sess")?
                .content
                .contains("latercascade")
        );
        assert_eq!(MemoryStore::open(&path)?.state.forgotten_ids, expected);

        // Own sources still cascade: a real owned observation and the generated explicit source.
        let generated = note.source_event_ids[0].clone();
        store.forget_for_account(&owner.token, &summary.memory_id)?;
        assert!(store.state.forgotten_ids.contains("victim-evt"));
        store.forget_for_account(&owner.token, &note.memory_id)?;
        assert!(store.state.forgotten_ids.contains(&generated));
        let reopened = MemoryStore::open(&path)?;
        assert!(reopened.state.forgotten_ids.contains(&generated));

        // Old data: an owned memory whose generated source has no owner record. The memory is
        // forgotten by its own id; the source is neither tombstoned nor claimed.
        let mut old = linked_memory("old", "memory-unrelated");
        old.supersedes = None;
        old.source_event_ids = vec!["event-old-source".to_owned()];
        store.state.memories.push(old);
        store.state.memory_owners.push(MemoryOwnerRecord {
            memory_id: "memory-link-old".to_owned(),
            person_id: owner.account.person_id.clone(),
        });
        store.persist()?;
        store.forget_for_account(&owner.token, "memory-link-old")?;
        assert!(store.state.forgotten_ids.contains("memory-link-old"));
        assert!(!store.state.forgotten_ids.contains("event-old-source"));
        assert!(
            !store
                .state
                .observation_owners
                .iter()
                .any(|r| r.event_id == "event-old-source")
        );
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn generated_explicit_source_is_owned_without_a_row_and_rolls_back() -> Result<(), StoreError> {
        let dir = temp_store_dir("generated-source");
        let path = dir.join("store.json");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let other = store.register_account("other@example.test", "correct horse battery staple")?;
        let device = store.register_device("dev-gen", &other.account.person_id, None, "Other")?;
        let note = remember_owned(&mut store, &owner.token, "generated source note")?;
        let source = note.source_event_ids[0].clone();
        assert!(store.observation_owned_by(&source, &owner.account.person_id));
        assert!(
            store.observations().all(|o| o.event_id != source),
            "no fake observation row"
        );
        assert!(MemoryStore::open(&path)?.observation_owned_by(&source, &owner.account.person_id));

        // The owned id cannot be taken by another account's capture, device push or import.
        store.start_session_for_account(&other.token, "gen-sess", context())?;
        let input = observation_for("gen-sess", &source, "squat attempt");
        let expected = format!("event belongs to another person: {source}");
        for error in [
            store
                .capture_for_account(&other.token, input.clone())
                .unwrap_err(),
            store.sync_push(&device.token, input).unwrap_err(),
        ] {
            assert!(matches!(&error, StoreError::Validation(text) if text == &expected));
        }
        assert!(store.observations().all(|o| o.event_id != source));
        let mut row = linked_memory("gen", "memory-unrelated");
        row.supersedes = None;
        row.memory_id = source.clone();
        assert!(import_forgotten(&mut store, &other.token, vec![row], &[]).is_err());
        assert!(
            store
                .export_for_account(&owner.token)?
                .observations
                .is_empty()
        );

        // A failed rename leaves neither the memory nor its owner records.
        let before = fs::read(&path)?;
        let owners = (
            store.state.memory_owners.len(),
            store.state.observation_owners.len(),
        );
        let mut blocked = HeldStoreDir::block(&path);
        let failed = store
            .remember_for_account(
                &owner.token,
                "blocked note",
                Vec::new(),
                context(),
                LifecycleInput::default(),
                MemoryKind::Explicit,
            )
            .expect_err("a blocked rename fails the remember");
        assert!(matches!(failed, StoreError::Io(_)));
        assert_eq!(
            (
                store.state.memory_owners.len(),
                store.state.observation_owners.len()
            ),
            owners
        );
        assert!(
            store
                .state
                .memories
                .iter()
                .all(|m| m.content != "blocked note")
        );
        blocked.release();
        assert_eq!(fs::read(&path)?, before);
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    // A revocation report names the revoked member by email as well as person id.
    #[test]
    fn project_revocations_include_the_member_email() -> Result<(), StoreError> {
        let path = temp_store_path("revocation-email");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let member =
            store.register_account("member@example.test", "correct horse battery staple")?;
        let (project_id, mine) = owned_project(&mut store, &owner.token, "revocation email")?;
        join_project(&mut store, &member.token, &project_id)?;
        store.share_project_memory(
            &owner.token,
            &project_id,
            &mine.memory_id,
            "member@example.test",
        )?;
        store.revoke_project_share(
            &owner.token,
            &project_id,
            &mine.memory_id,
            "member@example.test",
        )?;
        for token in [&owner.token, &member.token] {
            let revoked = store.project_revocations(token, &project_id)?;
            assert_eq!(revoked.len(), 1);
            assert_eq!(
                revoked[0].member_email.as_deref(),
                Some("member@example.test")
            );
            assert_eq!(revoked[0].member_person_id, member.account.person_id);
        }
        let _ = fs::remove_file(&path);
        Ok(())
    }

    // `project_maintenance` is owner-only. Members read what is explicitly shared with them.
    #[test]
    fn project_maintenance_is_owner_only_and_needs_an_owned_project() -> Result<(), StoreError> {
        let path = temp_store_path("maintenance-owner");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let member =
            store.register_account("member@example.test", "correct horse battery staple")?;
        let outsider =
            store.register_account("outsider@example.test", "correct horse battery staple")?;
        let (project_id, mine) =
            owned_project(&mut store, &owner.token, "maintain owner decision")?;
        join_project(&mut store, &member.token, &project_id)?;
        store.share_project_memory(
            &owner.token,
            &project_id,
            &mine.memory_id,
            "member@example.test",
        )?;
        store.create_project_team(&owner.token, &project_id, "Core")?;
        let before = fs::read(&path)?;

        // Owner: unchanged report.
        let report = store.project_maintenance(&owner.token, &project_id, Some("other-commit"))?;
        assert_eq!(report.active_memory_ids, vec![mine.memory_id.clone()]);
        assert_eq!(report.stale_memory_ids, vec![mine.memory_id.clone()]);

        // A current member and an outsider who know the id are refused, and
        // the error carries no memory id or commit.
        for token in [&member.token, &outsider.token] {
            let error = store
                .project_maintenance(token, &project_id, Some("other-commit"))
                .expect_err("only the owner reads maintenance");
            assert!(
                matches!(&error, StoreError::Validation(text) if text == "account does not own the project")
            );
        }
        assert_eq!(fs::read(&path)?, before, "a read never writes");
        let error = store
            .project_maintenance(&owner.token, "project-missing", None)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown project: project-missing")
        );
        assert!(
            store
                .project_maintenance("not-a-token", &project_id, None)
                .unwrap_err()
                .to_string()
                .contains("account authentication failed")
        );

        // A project with no owner record is not maintainable by any account until the
        // existing registration claims it; there is no other claim.
        store
            .state
            .project_owners
            .retain(|record| record.project_id != project_id);
        for token in [&owner.token, &outsider.token] {
            let error = store
                .project_maintenance(token, &project_id, None)
                .expect_err("an ownerless project is denied");
            assert!(
                matches!(&error, StoreError::Validation(text) if text == "account does not own the project")
            );
        }
        store.register_project(&owner.token, "cairn-demo", "Demo", None)?;
        assert!(
            store
                .project_maintenance(&owner.token, &project_id, None)
                .is_ok()
        );
        assert!(
            store
                .project_maintenance(&outsider.token, &project_id, None)
                .is_err()
        );
        fs::remove_file(path)?;
        Ok(())
    }

    // The Layer 4 registry (branches, worktrees, entities, freshness, merge, close) is
    // owner-only, and a merge cannot copy another account's memory.
    #[test]
    fn project_registry_is_owner_only_for_reads_and_writes() -> Result<(), StoreError> {
        const DENIED: &str = "account does not own the project";
        let path = temp_store_path("registry-owner");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let member =
            store.register_account("member@example.test", "correct horse battery staple")?;
        let outsider =
            store.register_account("outsider@example.test", "correct horse battery staple")?;
        let (project_id, shared) =
            owned_project(&mut store, &owner.token, "registry owner decision")?;
        join_project(&mut store, &member.token, &project_id)?;
        store.share_project_memory(
            &owner.token,
            &project_id,
            &shared.memory_id,
            "member@example.test",
        )?;
        store.create_project_team(&owner.token, &project_id, "Core")?;
        let branch =
            store.register_branch(&owner.token, &project_id, "feat/auth", None, None, None)?;
        let source = remember_with_context(&mut store, &owner.token, "registry merge source")?;
        let entity = || ProjectEntityInput {
            kind: "file".to_owned(),
            key: "src/secret.rs".to_owned(),
            content: None,
            source_memory_id: None,
            commit: None,
        };
        store.register_project_entity(&owner.token, &project_id, entity())?;
        let before = fs::read(&path)?;

        type Op = fn(&mut MemoryStore, &str, &str, &str, &str) -> Result<(), StoreError>;
        let ops: [(&str, Op); 9] = [
            ("register_branch", |s, t, p, _, _| {
                s.register_branch(t, p, "feat/evil", None, None, None)
                    .map(drop)
            }),
            ("branches", |s, t, p, _, _| s.branches(t, p).map(drop)),
            ("register_worktree", |s, t, p, _, _| {
                s.register_worktree(t, p, None, "/wt/evil", None).map(drop)
            }),
            ("worktrees", |s, t, p, _, _| s.worktrees(t, p).map(drop)),
            ("project_entities", |s, t, p, _, _| {
                s.project_entities(t, p).map(drop)
            }),
            ("register_project_entity", |s, t, p, _, _| {
                let input = ProjectEntityInput {
                    kind: "file".to_owned(),
                    key: "evil.rs".to_owned(),
                    content: None,
                    source_memory_id: None,
                    commit: None,
                };
                s.register_project_entity(t, p, input).map(drop)
            }),
            ("freshness_history", |s, t, p, _, _| {
                s.freshness_history(t, p).map(drop)
            }),
            ("close_branch", |s, t, p, b, _| {
                s.close_branch(t, p, b, "closing").map(drop)
            }),
            ("confirm_branch_merge", |s, t, p, b, m| {
                s.confirm_branch_merge(t, p, b, m, "abc123", "merge")
                    .map(drop)
            }),
        ];
        // A current member and an outsider who know every id are refused on
        // every operation, with one error that carries no registry content, and nothing is written.
        for (name, op) in &ops {
            for token in [&member.token, &outsider.token] {
                let error = op(
                    &mut store,
                    token,
                    &project_id,
                    &branch.branch_id,
                    &source.memory_id,
                )
                .expect_err(name);
                assert!(
                    matches!(&error, StoreError::Validation(text) if text == DENIED),
                    "{name}: {error}"
                );
            }
            let error = op(&mut store, "not-a-token", &project_id, "b", "m").unwrap_err();
            assert!(
                error.to_string().contains("account authentication failed"),
                "{name}"
            );
            let error = op(&mut store, &owner.token, "project-missing", "b", "m").unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("unknown project: project-missing"),
                "{name}"
            );
        }
        assert_eq!(fs::read(&path)?, before, "denials and errors write nothing");
        assert_eq!(
            MemoryStore::open(&path)?
                .branches(&owner.token, &project_id)?
                .len(),
            1
        );

        // The owner reads and writes normally.
        assert_eq!(store.branches(&owner.token, &project_id)?.len(), 1);
        store.register_worktree(
            &owner.token,
            &project_id,
            Some(branch.branch_id.clone()),
            "/wt/ok",
            None,
        )?;
        assert_eq!(store.worktrees(&owner.token, &project_id)?.len(), 1);
        assert!(
            !store
                .project_entities(&owner.token, &project_id)?
                .is_empty()
        );
        assert!(
            !store
                .freshness_history(&owner.token, &project_id)?
                .is_empty()
        );
        let merged = store.confirm_branch_merge(
            &owner.token,
            &project_id,
            &branch.branch_id,
            &source.memory_id,
            "abc123",
            "merge",
        )?;
        assert_eq!(merged.branch.status, "merged");
        let other =
            store.register_branch(&owner.token, &project_id, "feat/old", None, None, None)?;
        assert_eq!(
            store
                .close_branch(&owner.token, &project_id, &other.branch_id, "done")?
                .status,
            "closed"
        );

        // A merge cannot copy another account's memory into the caller's own project.
        let own = store
            .register_project(&outsider.token, "cairn-demo", "Mine", None)?
            .project_id;
        let own_branch =
            store.register_branch(&outsider.token, &own, "feat/auth", None, None, None)?;
        let victim = remember_with_context(&mut store, &owner.token, "registrymerge private text")?;
        let error = store
            .confirm_branch_merge(
                &outsider.token,
                &own,
                &own_branch.branch_id,
                &victim.memory_id,
                "abc123",
                "merge",
            )
            .expect_err("another account's memory is not a merge source");
        assert_unknown_id(&error, &victim.memory_id);
        assert!(store.project_memories(&outsider.token, &own)?.is_empty());
        assert_eq!(store.branches(&outsider.token, &own)?[0].status, "active");
        // A branch id from another project does not resolve in the caller's project.
        assert!(
            store
                .register_worktree(
                    &outsider.token,
                    &own,
                    Some(branch.branch_id.clone()),
                    "/wt/x",
                    None
                )
                .is_err()
        );

        // A project with no owner record is not registry-accessible until registration claims it.
        store
            .state
            .project_owners
            .retain(|record| record.project_id != project_id);
        assert!(store.branches(&owner.token, &project_id).is_err());
        assert!(
            store
                .register_branch(&outsider.token, &project_id, "feat/z", None, None, None)
                .is_err()
        );
        store.register_project(&owner.token, "cairn-demo", "Demo", None)?;
        assert!(store.branches(&owner.token, &project_id).is_ok());
        assert!(store.branches(&outsider.token, &project_id).is_err());
        fs::remove_file(path)?;
        Ok(())
    }

    // CM-AUDIT-015 B1: a merge confirmation must not use a stored foreign row as the promotion.
    #[test]
    fn stored_foreign_project_rows_do_not_alias_a_branch_merge() -> Result<(), StoreError> {
        let path = temp_store_path("merge-foreign-row");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let outsider =
            store.register_account("outsider@example.test", "correct horse battery staple")?;
        let (project_id, _) = owned_project(&mut store, &owner.token, "merge first decision")?;
        let branch =
            store.register_branch(&owner.token, &project_id, "feat/auth", None, None, None)?;
        let source = remember_with_context(&mut store, &owner.token, "merge owner source")?;
        let mut legacy_ctx = context();
        legacy_ctx.branch = Some("feat/legacy".to_owned());
        let legacy_branch =
            store.register_branch(&owner.token, &project_id, "feat/legacy", None, None, None)?;
        let legacy_source = store.remember_for_account(
            &owner.token,
            "merge legacy source",
            Vec::new(),
            legacy_ctx,
            LifecycleInput::default(),
            MemoryKind::Explicit,
        )?;

        // Stored before the import rule: a foreign-owned row naming the owner's unmerged
        // source, and an unowned local row naming another source.
        let mut foreign = project_row("memory-foreign-merge", &project_id, "foreignmerge text");
        foreign.promoted_from = Some(source.memory_id.clone());
        let mut local = project_row("memory-local-merge", &project_id, "localmerge text");
        local.promoted_from = Some(legacy_source.memory_id.clone());
        store.state.memory_owners.push(MemoryOwnerRecord {
            memory_id: foreign.memory_id.clone(),
            person_id: outsider.account.person_id.clone(),
        });
        let foreign_before = foreign.clone();
        store.state.memories.extend([foreign, local]);
        store.persist()?;

        let merged = store.confirm_branch_merge(
            &owner.token,
            &project_id,
            &branch.branch_id,
            &source.memory_id,
            "mc1",
            "merge",
        )?;
        assert_ne!(merged.memory.memory_id, "memory-foreign-merge");
        assert_eq!(
            merged.branch.merge_memory_id.as_deref(),
            Some(merged.memory.memory_id.as_str())
        );
        assert!(store.memory_owned_by(&merged.memory.memory_id, &owner.account.person_id));
        assert_eq!(
            merged.memory.promoted_from.as_deref(),
            Some(source.memory_id.as_str())
        );
        let listed = store.project_memories(&owner.token, &project_id)?;
        assert!(
            listed
                .iter()
                .any(|m| m.memory_id == merged.memory.memory_id)
        );
        assert!(listed.iter().all(|m| m.memory_id != "memory-foreign-merge"));

        // The foreign row is untouched: content, commit, freshness and owner; its owner still exports it.
        let stored = store
            .state
            .memories
            .iter()
            .find(|m| m.memory_id == "memory-foreign-merge")
            .expect("kept");
        assert_eq!(stored, &foreign_before);
        assert!(store.memory_owned_by("memory-foreign-merge", &outsider.account.person_id));
        assert!(
            store
                .export_for_account(&outsider.token)?
                .memories
                .iter()
                .any(|m| m.memory_id == "memory-foreign-merge")
        );
        assert!(
            store
                .freshness_history(&owner.token, &project_id)?
                .iter()
                .all(|event| event.memory_id != "memory-foreign-merge")
        );
        assert!(
            store
                .project_entities(&owner.token, &project_id)?
                .iter()
                .all(|entity| !entity.key.contains("memory-foreign-merge")
                    && entity.source_memory_id.as_deref() != Some("memory-foreign-merge"))
        );
        // The owner's repeated merge is idempotent.
        let again = store.confirm_branch_merge(
            &owner.token,
            &project_id,
            &branch.branch_id,
            &source.memory_id,
            "mc1",
            "merge",
        )?;
        assert_eq!(again.memory.memory_id, merged.memory.memory_id);

        // Control: an unowned local row naming the source is still the idempotent promotion.
        let local_merge = store.confirm_branch_merge(
            &owner.token,
            &project_id,
            &legacy_branch.branch_id,
            &legacy_source.memory_id,
            "mc2",
            "merge",
        )?;
        assert_eq!(local_merge.memory.memory_id, "memory-local-merge");
        let local_again = store.confirm_branch_merge(
            &owner.token,
            &project_id,
            &legacy_branch.branch_id,
            &legacy_source.memory_id,
            "mc2",
            "merge",
        )?;
        assert_eq!(local_again.memory.memory_id, "memory-local-merge");
        fs::remove_file(path)?;
        Ok(())
    }

    // CM-AUDIT-015 R1: a branch merged to a foreign row by the old behaviour is refused, not read.
    #[test]
    fn merged_branch_aliased_to_a_foreign_row_is_not_returned() -> Result<(), StoreError> {
        let path = temp_store_path("merge-aliased-row");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let outsider =
            store.register_account("outsider@example.test", "correct horse battery staple")?;
        let (project_id, _) = owned_project(&mut store, &owner.token, "alias first decision")?;
        let branch =
            store.register_branch(&owner.token, &project_id, "feat/auth", None, None, None)?;
        let source = remember_with_context(&mut store, &owner.token, "alias owner source")?;
        let mut foreign = project_row("memory-foreign-alias", &project_id, "foreignalias text");
        foreign.promoted_from = Some(source.memory_id.clone());
        // The same shape in another project of the same owner: also not this project's row.
        let other_project = store
            .register_project(&owner.token, "other-repo", "Other", None)?
            .project_id;
        let mut elsewhere = project_row(
            "memory-elsewhere-alias",
            &other_project,
            "elsewherealias text",
        );
        elsewhere.promoted_from = Some(source.memory_id.clone());
        store.state.memory_owners.push(MemoryOwnerRecord {
            memory_id: foreign.memory_id.clone(),
            person_id: outsider.account.person_id.clone(),
        });
        store.state.memories.extend([foreign, elsewhere]);
        // Old state: the branch was marked merged and aliased to the foreign row.
        for target in ["memory-foreign-alias", "memory-elsewhere-alias"] {
            let stored = store
                .state
                .project_branches
                .iter_mut()
                .find(|b| b.branch_id == branch.branch_id)
                .expect("branch");
            stored.status = "merged".to_owned();
            stored.merge_commit = Some("mc1".to_owned());
            stored.merge_memory_id = Some(target.to_owned());
            store.persist()?;
            let before = fs::read(&path)?;
            let error = store
                .confirm_branch_merge(
                    &owner.token,
                    &project_id,
                    &branch.branch_id,
                    &source.memory_id,
                    "mc1",
                    "merge",
                )
                .expect_err("the aliased row is not returned");
            assert!(
                matches!(&error, StoreError::Validation(text) if text == "project branch is already merged")
            );
            assert!(!error.to_string().contains("alias text"));
            assert_eq!(fs::read(&path)?, before, "nothing is written or repaired");
            assert_eq!(
                store.branches(&owner.token, &project_id)?[0]
                    .merge_memory_id
                    .as_deref(),
                Some(target)
            );
        }
        assert!(
            store
                .export_for_account(&outsider.token)?
                .memories
                .iter()
                .any(|m| m.memory_id == "memory-foreign-alias")
        );
        // Controls: the outsider is still refused, and a bad token fails on authentication.
        assert!(
            store
                .confirm_branch_merge(
                    &outsider.token,
                    &project_id,
                    &branch.branch_id,
                    &source.memory_id,
                    "mc1",
                    "merge"
                )
                .is_err()
        );
        assert!(
            store
                .confirm_branch_merge(
                    "not-a-token",
                    &project_id,
                    &branch.branch_id,
                    &source.memory_id,
                    "mc1",
                    "merge"
                )
                .unwrap_err()
                .to_string()
                .contains("account authentication failed")
        );
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn unscoped_import_still_accepts_any_tombstone() -> Result<(), StoreError> {
        let path = temp_store_path("unscoped-tombstone");
        let mut store = MemoryStore::open(&path)?;
        let mut bundle = store.export();
        bundle.forgotten_ids = vec!["unscoped-unknown".to_owned()];
        assert_eq!(store.import_bundle(bundle)?.imported_forgotten_ids, 1);
        fs::remove_file(path)?;
        Ok(())
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
    fn why_changed_priority_matches_the_recall_result_fields() -> Result<(), StoreError> {
        let path = temp_store_path("why-priority");
        let mut store = MemoryStore::open(&path)?;
        let context = WorkContext {
            files: vec!["src/a.rs".to_owned(), "src/b.rs".to_owned()],
            symbols: vec!["A::run".to_owned()],
            ..WorkContext::default()
        };
        let with_reason = store.remember("Chose A because it is faster.", Vec::new(), context)?;
        let plain = store.remember("Plain note", Vec::new(), WorkContext::default())?;
        for memory in [&with_reason, &plain] {
            let result = recall_result(memory, 1.0, &[], &HashSet::new(), &RecallFilter::default());
            assert_eq!(
                why_changed_priority(memory),
                (result.decision_reason.is_some(), result.relationships.len())
            );
        }
        assert_eq!(why_changed_priority(&plain), (false, 0));
        assert!(why_changed_priority(&with_reason).0 && why_changed_priority(&with_reason).1 > 0);
        let _ = fs::remove_file(path);
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
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let person_id = owner.account.person_id.clone();
        let first = store.register_device(
            "device-a",
            person_id.clone(),
            Some("agent-a".to_owned()),
            "MacBook",
        )?;
        let second = store.register_device("device-b", person_id, None, "Laptop")?;
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

        let rejected = restored
            .rotate_device_token(&first.token, "device-a")
            .expect_err("a device token cannot mint tokens");
        assert!(rejected.to_string().contains(WEB_ONLY_ERROR));
        let rotated = restored.rotate_device_token(&owner.token, "device-a")?;
        assert_ne!(rotated.token, first.token);
        assert!(
            restored
                .sync_push(
                    &first.token,
                    observation_for("sync-session", "sync-event-old-token", "old token"),
                )
                .is_err()
        );

        let revoked = restored.revoke_device(&owner.token, "device-b")?;
        assert_eq!(revoked.status, DeviceStatus::Revoked);
        assert!(restored.sync_pull(&second.token, pulled.cursor).is_err());
        assert!(restored.authenticate_device(&rotated.token).is_ok());
        assert!(restored.authenticate_device(&second.token).is_err());

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn device_token_is_an_agent_credential_scoped_to_its_account() -> Result<(), StoreError> {
        let path = temp_store_path("device-agent");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let other = store.register_account("other@example.test", "correct horse battery staple")?;
        let device = store.register_device(
            "device-own",
            owner.account.person_id.clone(),
            None,
            "Laptop",
        )?;
        let other_device = store.register_device(
            "device-other",
            other.account.person_id.clone(),
            None,
            "Other",
        )?;

        // Agent actions run as the owning account and stay inside it.
        let (project_id, _) = owned_project(&mut store, &device.token, "device made decision")?;
        assert_eq!(
            store.authenticate_account(&device.token)?.person_id,
            owner.account.person_id
        );
        assert_eq!(store.project_memories(&owner.token, &project_id)?.len(), 1);
        let foreign = store
            .project_memories(&other_device.token, &project_id)
            .expect_err("another account's device is outside the project");
        assert!(foreign.to_string().contains("does not own"));

        // A device push is owned by the account, so account reads and session end see it.
        store.start_session_for_account(&device.token, "device-session", context())?;
        store.sync_push(
            &device.token,
            observation_for("device-session", "device-event", "device pushed marker"),
        )?;
        assert!(store.observation_owned_by("device-event", &owner.account.person_id));
        store.end_session_for_account(&owner.token, "device-session")?;

        // Web-only actions refuse a device token with the web-only error.
        let web_only = [
            store.authenticate_session(&device.token).map(|_| ()),
            store.audit_events(&device.token, 10).map(|_| ()),
            store
                .rotate_device_token(&device.token, "device-own")
                .map(|_| ()),
            store.revoke_device(&device.token, "device-own").map(|_| ()),
            store.logout_account(&device.token),
        ];
        for result in web_only {
            let error = result.expect_err("device token is refused");
            assert!(error.to_string().contains(WEB_ONLY_ERROR));
        }
        assert!(store.authenticate_session(&owner.token).is_ok());

        // A solo project merges with a device token; a shared project does not.
        let branch =
            store.register_branch(&device.token, &project_id, "feat/auth", None, None, None)?;
        let source = remember_with_context(&mut store, &device.token, "solo merge source")?;
        store.confirm_branch_merge(
            &device.token,
            &project_id,
            &branch.branch_id,
            &source.memory_id,
            "c1",
            "solo",
        )?;
        join_project(&mut store, &other.token, &project_id)?;
        let shared_branch =
            store.register_branch(&owner.token, &project_id, "feat/shared", None, None, None)?;
        let mut shared_context = context();
        shared_context.branch = Some("feat/shared".to_owned());
        let shared_source = store.remember_for_account(
            &owner.token,
            "shared merge source",
            Vec::new(),
            shared_context,
            LifecycleInput::default(),
            MemoryKind::Explicit,
        )?;
        let refused = store
            .confirm_branch_merge(
                &device.token,
                &project_id,
                &shared_branch.branch_id,
                &shared_source.memory_id,
                "c2",
                "shared",
            )
            .expect_err("a shared project needs the web console");
        assert!(refused.to_string().contains(WEB_ONLY_ERROR));
        store.confirm_branch_merge(
            &owner.token,
            &project_id,
            &shared_branch.branch_id,
            &shared_source.memory_id,
            "c2",
            "shared",
        )?;

        // A revoked token is rejected, and use is recorded only for an active one.
        store.touch_device(&device.token);
        let used = store
            .devices()
            .find(|candidate| candidate.device_id == "device-own")
            .map(|candidate| candidate.last_seen_at.clone());
        assert!(used.is_some());
        store.revoke_device(&owner.token, "device-own")?;
        let revoked = store
            .authenticate_account(&device.token)
            .expect_err("revoked device token");
        assert!(revoked.to_string().contains("device is revoked"));
        assert!(store.is_device_token(&device.token));
        assert!(!store.is_device_token(&owner.token));

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

    fn empty_capture(session_id: &str, event_id: &str, content: &str) -> ObservationInput {
        ObservationInput {
            context: WorkContext::default(),
            ..observation_for(session_id, event_id, content)
        }
    }

    /// Makes the account a project member the way the committed project file does: one
    /// capture that carries the project id.
    fn join_project(
        store: &mut MemoryStore,
        token: &str,
        project_id: &str,
    ) -> Result<(), StoreError> {
        let event_id = format!("join-{project_id}-{}", &hash_token(token)[..8]);
        store.capture_for_account(
            token,
            ObservationInput {
                context: WorkContext {
                    project_id: Some(project_id.to_owned()),
                    ..WorkContext::default()
                },
                ..observation_for("join", &event_id, "joined by project file")
            },
        )?;
        Ok(())
    }

    #[test]
    fn account_sessions_isolate_context_for_one_public_id() -> Result<(), StoreError> {
        let path = temp_store_path("session-person");
        fs::write(
            &path,
            r#"{"version":1,"sessions":[{"session_id":"shared-session","context":{"repository":"legacy-repo","branch":"legacy-branch","worktree":"legacy-tree","commit":null},"started_at":"1","ended_at":null}]}"#,
        )?;
        let mut store = MemoryStore::open(&path)?;
        let legacy = store
            .sessions()
            .find(|session| session.session_id == "shared-session")
            .cloned()
            .expect("legacy session");
        assert_eq!(legacy.person_id, None);
        assert_eq!(legacy.context.branch.as_deref(), Some("legacy-branch"));
        assert!(
            serde_json::to_value(&legacy)
                .expect("session json")
                .get("person_id")
                .is_none()
        );

        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let member =
            store.register_account("member@example.test", "correct horse battery staple")?;
        let owner_context = WorkContext {
            repository: Some("owner-repo".to_owned()),
            branch: Some("owner-branch".to_owned()),
            worktree: Some("owner-tree".to_owned()),
            ..WorkContext::default()
        };
        let member_context = WorkContext {
            repository: Some("member-repo".to_owned()),
            branch: Some("member-branch".to_owned()),
            ..WorkContext::default()
        };
        let session_id = "shared-session";
        let started =
            store.start_session_for_account(&owner.token, session_id, owner_context.clone())?;
        assert_eq!(started.context, owner_context);
        assert_eq!(
            started.person_id.as_deref(),
            Some(owner.account.person_id.as_str())
        );
        let retry =
            store.start_session_for_account(&owner.token, session_id, member_context.clone())?;
        assert_eq!(retry.context, owner_context);
        assert!(retry.ended_at.is_none());
        let member_started =
            store.start_session_for_account(&member.token, session_id, member_context.clone())?;
        assert_eq!(member_started.context, member_context);
        let unscoped = store.start_session(session_id, owner_context.clone())?;
        assert_eq!(unscoped.person_id, None);
        assert_eq!(unscoped.context.branch.as_deref(), Some("legacy-branch"));
        assert_eq!(
            store
                .sessions()
                .filter(|session| session.session_id == session_id)
                .count(),
            3
        );

        let owner_obs = store.capture_for_account(
            &owner.token,
            empty_capture(session_id, "owner-evt", "alphaquux ownerfact"),
        )?;
        assert_eq!(
            owner_obs.observation.context.branch.as_deref(),
            Some("owner-branch")
        );
        assert_eq!(
            owner_obs.observation.context.worktree.as_deref(),
            Some("owner-tree")
        );
        let sent = WorkContext {
            files: vec!["member-file.rs".to_owned()],
            ..WorkContext::default()
        };
        let member_sent = store.capture_for_account(
            &member.token,
            ObservationInput {
                context: sent.clone(),
                ..observation_for(session_id, "member-evt", "betazet memberfact")
            },
        )?;
        assert_eq!(member_sent.observation.context, sent);
        let member_empty = store.capture_for_account(
            &member.token,
            empty_capture(session_id, "member-empty", "memberemptyfact"),
        )?;
        assert_eq!(
            member_empty.observation.context.branch.as_deref(),
            Some("member-branch")
        );
        assert!(member_empty.observation.context.worktree.is_none());
        let legacy_obs =
            store.capture_observation(empty_capture(session_id, "legacy-evt", "legacyfact"))?;
        assert_eq!(
            legacy_obs.observation.context.branch.as_deref(),
            Some("legacy-branch")
        );
        let no_session = store.capture_for_account(
            &owner.token,
            empty_capture("never-started", "no-session-evt", "nosessionfact"),
        )?;
        assert_eq!(no_session.observation.context, WorkContext::default());
        assert!(
            store
                .sessions()
                .all(|session| session.session_id != "never-started")
        );

        store.start_session(
            "unscoped-only",
            WorkContext {
                branch: Some("only-unscoped".to_owned()),
                ..WorkContext::default()
            },
        )?;
        let missing = store
            .end_session_for_account(&owner.token, "unscoped-only")
            .expect_err("an account does not close the unscoped row");
        assert!(
            missing
                .to_string()
                .contains("unknown session: unscoped-only")
        );
        assert!(
            store
                .sessions()
                .find(|session| session.session_id == "unscoped-only")
                .expect("unscoped row")
                .ended_at
                .is_none()
        );
        store.start_session_for_account(&owner.token, "account-only", owner_context.clone())?;
        let missing = store
            .end_session("account-only")
            .expect_err("an unscoped end does not close an account row");
        assert!(
            missing
                .to_string()
                .contains("unknown session: account-only")
        );
        let missing = store
            .end_session_for_account(&member.token, "account-only")
            .expect_err("another account does not close this row");
        assert!(
            missing
                .to_string()
                .contains("unknown session: account-only")
        );
        assert!(
            store
                .sessions()
                .find(|session| session.session_id == "account-only")
                .expect("account row")
                .ended_at
                .is_none()
        );

        let owner_summary = store.end_session_for_account(&owner.token, session_id)?;
        assert_eq!(owner_summary.scope, MemoryScope::Worktree);
        assert_eq!(
            owner_summary.context.branch.as_deref(),
            Some("owner-branch")
        );
        assert!(owner_summary.content.contains("ownerfact"));
        assert!(!owner_summary.content.contains("memberfact"));
        assert!(!owner_summary.content.contains("legacyfact"));
        assert!(
            store
                .sessions()
                .find(|session| {
                    session.session_id == session_id
                        && session.person_id == member_started.person_id
                })
                .expect("member row")
                .ended_at
                .is_none()
        );
        let resumed =
            store.start_session_for_account(&owner.token, session_id, member_context.clone())?;
        assert_eq!(resumed.context, owner_context);
        assert!(resumed.ended_at.is_some());
        let member_summary = store.end_session_for_account(&member.token, session_id)?;
        assert_eq!(member_summary.scope, MemoryScope::Branch);
        assert_eq!(
            member_summary.context.repository.as_deref(),
            Some("member-repo")
        );
        assert!(member_summary.content.contains("memberfact"));
        assert!(!member_summary.content.contains("ownerfact"));
        assert!(
            store
                .sessions()
                .find(|session| session.session_id == session_id && session.person_id.is_none())
                .expect("legacy row")
                .ended_at
                .is_none()
        );
        let legacy_summary = store.end_session(session_id)?;
        assert_eq!(legacy_summary.memory_id, format!("memory-{session_id}"));
        assert_eq!(legacy_summary.scope, MemoryScope::Worktree);
        assert!(legacy_summary.content.contains("legacyfact"));
        assert!(!legacy_summary.content.contains("ownerfact"));

        let owner_hits =
            store.search_for_account(&owner.token, "ownerfact", 5, &RecallFilter::default())?;
        assert_eq!(owner_hits.len(), 1);
        assert_eq!(
            owner_hits[0].context.branch.as_deref(),
            Some("owner-branch")
        );
        assert!(
            store
                .search_for_account(&member.token, "ownerfact", 5, &RecallFilter::default())?
                .is_empty()
        );

        let owner_export = store.export_for_account(&owner.token)?;
        assert!(owner_export.sessions.iter().all(|session| {
            session.person_id.as_deref() == Some(owner.account.person_id.as_str())
        }));
        assert!(
            owner_export
                .sessions
                .iter()
                .any(|session| session.context == owner_context)
        );
        assert!(
            owner_export
                .sessions
                .iter()
                .all(
                    |session| session.context.branch.as_deref() != Some("member-branch")
                        && session.context.branch.as_deref() != Some("legacy-branch")
                )
        );
        assert!(
            owner_export
                .observations
                .iter()
                .all(|observation| observation.event_id != "member-evt"
                    && observation.event_id != "legacy-evt")
        );
        let member_export = store.export_for_account(&member.token)?;
        assert!(member_export.sessions.iter().all(|session| {
            session.person_id.as_deref() == Some(member.account.person_id.as_str())
                && session.context.repository.as_deref() != Some("owner-repo")
        }));
        let unscoped_export = store.export();
        assert!(
            unscoped_export
                .sessions
                .iter()
                .all(|session| session.person_id.is_none())
        );
        assert!(
            unscoped_export
                .sessions
                .iter()
                .any(|session| session.context.branch.as_deref() == Some("legacy-branch"))
        );
        assert_eq!(
            store
                .import_for_account(&owner.token, owner_export.clone())?
                .imported_sessions,
            0
        );
        let mut foreign_sessions = owner_export.clone();
        foreign_sessions.observations.clear();
        foreign_sessions.memories.clear();
        foreign_sessions.forgotten_ids.clear();
        let session_count = store.sessions().count();
        let rejected = store
            .import_for_account(&member.token, foreign_sessions)
            .expect_err("a foreign person_id is not an authoritative session");
        assert!(
            rejected
                .to_string()
                .contains("account does not own the memory")
        );
        assert_eq!(store.sessions().count(), session_count);

        let mut old_bundle = owner_export.clone();
        for session in &mut old_bundle.sessions {
            session.person_id = None;
        }
        assert_eq!(
            store
                .import_for_account(&owner.token, old_bundle)?
                .imported_sessions,
            0
        );
        assert_eq!(
            store
                .sessions()
                .filter(|session| session.session_id == session_id && session.person_id.is_none())
                .count(),
            1
        );

        let restored_path = temp_store_path("session-person-import");
        let mut restored = MemoryStore::open(&restored_path)?;
        let foreign_bundle = owner_export.clone();
        restored.import_bundle(owner_export)?;
        restored.import_bundle(member_export)?;
        assert!(restored.sessions().any(|session| {
            session.person_id.as_deref() == Some(owner.account.person_id.as_str())
                && session.context == owner_context
        }));
        assert!(restored.sessions().any(|session| {
            session.person_id.as_deref() == Some(member.account.person_id.as_str())
                && session.context == member_context
        }));
        let fresh_path = temp_store_path("session-person-foreign");
        let mut fresh = MemoryStore::open(&fresh_path)?;
        let stranger =
            fresh.bootstrap_account("stranger@example.test", "correct horse battery staple")?;
        let rejected = fresh
            .import_for_account(&stranger.token, foreign_bundle)
            .expect_err("a foreign person_id is not an authoritative session");
        assert!(
            rejected
                .to_string()
                .contains("account does not own the memory")
        );
        assert_eq!(fresh.sessions().count(), 0);
        let bound = fresh.import_for_account(&stranger.token, unscoped_export)?;
        assert!(bound.imported_sessions >= 1);
        assert!(fresh.sessions().all(|session| {
            session.person_id.as_deref() == Some(stranger.account.person_id.as_str())
        }));
        assert!(
            fresh
                .sessions()
                .any(|session| session.context.branch.as_deref() == Some("legacy-branch"))
        );

        let file: Value = serde_json::from_str(&fs::read_to_string(&path)?)?;
        assert_eq!(file["version"], 1);
        drop(store);
        let reopened = MemoryStore::open(&path)?;
        assert_eq!(
            reopened
                .sessions()
                .filter(|session| session.session_id == session_id)
                .count(),
            3
        );
        assert!(reopened.sessions().any(|session| {
            session.person_id.as_deref() == Some(owner.account.person_id.as_str())
                && session.context == owner_context
                && session.ended_at.is_some()
        }));
        assert!(reopened.sessions().any(|session| {
            session.person_id.as_deref() == Some(member.account.person_id.as_str())
                && session.context == member_context
                && session.ended_at.is_some()
        }));
        assert!(reopened.sessions().any(|session| {
            session.person_id.is_none()
                && session.context.branch.as_deref() == Some("legacy-branch")
                && session.ended_at.is_some()
        }));

        fs::remove_file(path)?;
        fs::remove_file(restored_path)?;
        fs::remove_file(fresh_path)?;
        Ok(())
    }

    #[test]
    fn two_accounts_keep_distinct_session_summaries_for_one_session_id() -> Result<(), StoreError> {
        let path = temp_store_path("shared-session");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let member =
            store.register_account("member@example.test", "correct horse battery staple")?;
        let session_id = "shared-session";
        store.start_session_for_account(&owner.token, session_id, WorkContext::default())?;
        store.start_session_for_account(&member.token, session_id, WorkContext::default())?;
        store.capture_for_account(
            &owner.token,
            observation_for(session_id, "owner-evt", "alphaquux ownerfact"),
        )?;
        store.capture_for_account(
            &member.token,
            observation_for(session_id, "member-evt", "betazet memberfact"),
        )?;
        let owner_summary = store.end_session_for_account(&owner.token, session_id)?;
        let member_summary = store.end_session_for_account(&member.token, session_id)?;
        assert_ne!(owner_summary.memory_id, member_summary.memory_id);
        assert_eq!(
            owner_summary.memory_id,
            format!("memory-{}-{session_id}", owner.account.person_id)
        );
        assert_eq!(
            member_summary.memory_id,
            format!("memory-{}-{session_id}", member.account.person_id)
        );
        assert!(owner_summary.content.contains("ownerfact"));
        assert!(!owner_summary.content.contains("memberfact"));
        let owner_hits =
            store.search_for_account(&owner.token, "ownerfact", 5, &RecallFilter::default())?;
        assert_eq!(owner_hits.len(), 1);
        assert_eq!(owner_hits[0].memory_id, owner_summary.memory_id);
        let member_hits =
            store.search_for_account(&member.token, "ownerfact", 5, &RecallFilter::default())?;
        assert!(member_hits.is_empty());
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn account_search_limits_owned_matches_and_hides_foreign_contradiction_ids()
    -> Result<(), StoreError> {
        let path = temp_store_path("account-search-limit");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let member =
            store.register_account("member@example.test", "correct horse battery staple")?;
        let filter = RecallFilter::default();
        let explicit = |store: &mut MemoryStore, token: Option<&str>, content: &str| match token {
            Some(token) => store.remember_for_account(
                token,
                content,
                Vec::new(),
                WorkContext::default(),
                LifecycleInput::default(),
                MemoryKind::Explicit,
            ),
            None => store.remember(content, Vec::new(), WorkContext::default()),
        };

        let foreign = explicit(
            &mut store,
            Some(&member.token),
            "ranktoken ranktoken ranktoken foreignmarker",
        )?;
        let _unowned = explicit(&mut store, None, "ranktoken ranktoken unownedmarker")?;
        std::thread::sleep(std::time::Duration::from_millis(2));
        let owned_early = explicit(&mut store, Some(&owner.token), "ranktoken ownedearly")?;
        std::thread::sleep(std::time::Duration::from_millis(2));
        let owned_late = explicit(&mut store, Some(&owner.token), "ranktoken ownedlate")?;
        assert!(owned_early.memory_id < owned_late.memory_id);

        let limit_one = store.search_for_account(&owner.token, "ranktoken", 1, &filter)?;
        assert_eq!(limit_one.len(), 1);
        assert_eq!(limit_one[0].memory_id, owned_early.memory_id);
        assert_eq!(limit_one[0].score, 1.0);
        assert!(!limit_one[0].content.contains("foreignmarker"));
        assert!(!limit_one[0].content.contains("unownedmarker"));

        let limit_two = store.search_for_account(&owner.token, "ranktoken", 2, &filter)?;
        assert_eq!(
            limit_two
                .iter()
                .map(|result| result.memory_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                owned_early.memory_id.as_str(),
                owned_late.memory_id.as_str()
            ]
        );
        let again = store.search_for_account(&owner.token, "ranktoken", 2, &filter)?;
        assert_eq!(again, limit_two);
        assert!(
            store
                .search_for_account(&owner.token, "ranktoken", 0, &filter)?
                .is_empty()
        );

        let global = store.search_with_filter("ranktoken", 1, &filter)?;
        assert_eq!(global.len(), 1);
        assert_eq!(global[0].memory_id, foreign.memory_id);

        std::thread::sleep(std::time::Duration::from_millis(2));
        let tie_foreign = explicit(&mut store, Some(&member.token), "tiealpha tieforeign")?;
        std::thread::sleep(std::time::Duration::from_millis(2));
        let tie_owned = explicit(&mut store, Some(&owner.token), "tiealpha tieowned")?;
        assert!(tie_foreign.memory_id < tie_owned.memory_id);
        let tie = store.search_for_account(&owner.token, "tiealpha", 1, &filter)?;
        assert_eq!(tie.len(), 1);
        assert_eq!(tie[0].memory_id, tie_owned.memory_id);
        assert!(!tie[0].content.contains("tieforeign"));

        let signal_owned = explicit(
            &mut store,
            Some(&owner.token),
            "signalbeta lattice does not apply",
        )?;
        let signal_foreign = explicit(
            &mut store,
            Some(&member.token),
            "signalbeta lattice does apply",
        )?;
        let _signal_unowned = explicit(&mut store, None, "signalbeta lattice does apply")?;
        let signal = store.search_for_account(&owner.token, "signalbeta", 5, &filter)?;
        assert_eq!(signal.len(), 1);
        assert_eq!(signal[0].memory_id, signal_owned.memory_id);
        assert_eq!(signal[0].content, "signalbeta lattice does not apply");
        assert!(!signal[0].contradictory);
        assert!(signal[0].contradiction_ids.is_empty());
        assert!(signal[0].decision_reason.is_none());
        assert!(signal[0].relationships.is_empty());
        let unscoped_signal = store.search_with_filter("signalbeta", 5, &filter)?;
        let unscoped_owned = unscoped_signal
            .iter()
            .find(|result| result.memory_id == signal_owned.memory_id)
            .unwrap();
        assert!(unscoped_owned.contradictory);
        assert!(
            unscoped_owned
                .contradiction_ids
                .contains(&signal_foreign.memory_id)
        );

        std::thread::sleep(std::time::Duration::from_millis(2));
        let quota_neg = explicit(
            &mut store,
            Some(&owner.token),
            "quotadelta widget does not apply",
        )?;
        std::thread::sleep(std::time::Duration::from_millis(2));
        let quota_pos = explicit(
            &mut store,
            Some(&owner.token),
            "quotadelta widget does apply",
        )?;
        let quota_foreign = explicit(
            &mut store,
            Some(&member.token),
            "quotadelta widget does apply",
        )?;
        assert!(quota_neg.memory_id < quota_pos.memory_id);
        let quota = store.search_for_account(&owner.token, "quotadelta", 5, &filter)?;
        assert_eq!(quota.len(), 2);
        let neg = quota
            .iter()
            .find(|result| result.memory_id == quota_neg.memory_id)
            .unwrap();
        let pos = quota
            .iter()
            .find(|result| result.memory_id == quota_pos.memory_id)
            .unwrap();
        assert!(neg.contradictory);
        assert_eq!(neg.content, "quotadelta widget does not apply");
        assert_eq!(neg.contradiction_ids, vec![quota_pos.memory_id.clone()]);
        assert!(pos.contradictory);
        assert_eq!(pos.content, "quotadelta widget does apply");
        assert_eq!(pos.contradiction_ids, vec![quota_neg.memory_id.clone()]);

        let context = store.context_for_account(&owner.token, "quotadelta", 1, &filter)?;
        assert_eq!(context.results.len(), 1);
        assert_eq!(context.results[0].memory_id, quota_neg.memory_id);
        assert_eq!(
            context.results[0].contradiction_ids,
            vec![quota_pos.memory_id.clone()]
        );
        assert!(context.results[0].contradictory);
        assert_eq!(context.omitted_count, 1);
        assert_eq!(
            context.omitted_memory_ids,
            vec![quota_pos.memory_id.clone()]
        );
        assert!(
            !context.results[0]
                .contradiction_ids
                .contains(&quota_foreign.memory_id)
        );

        let why = store.why_changed_for_account(&owner.token, "quotadelta", 5, &filter)?;
        assert_eq!(why.results.len(), 2);
        assert!(why.omitted_memory_ids.is_empty());
        assert!(why.results.iter().all(|result| {
            (result.memory_id == quota_neg.memory_id || result.memory_id == quota_pos.memory_id)
                && result.contradictory
                && !result.contradiction_ids.contains(&quota_foreign.memory_id)
                && result.content.contains("quotadelta widget")
        }));

        let rank_context = store.context_for_account(&owner.token, "ranktoken", 1, &filter)?;
        assert_eq!(rank_context.results[0].memory_id, owned_early.memory_id);
        assert_eq!(
            rank_context.omitted_memory_ids,
            vec![owned_late.memory_id.clone()]
        );

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn account_resume_and_unresolved_hide_foreign_contradiction_ids() -> Result<(), StoreError> {
        let path = temp_store_path("account-resume-contradiction");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let member =
            store.register_account("member@example.test", "correct horse battery staple")?;
        let filter = RecallFilter::default();
        let branch = WorkContext {
            repository: Some("cairn-demo".to_owned()),
            branch: Some("main".to_owned()),
            ..WorkContext::default()
        };
        let capture = |store: &mut MemoryStore,
                       token: &str,
                       session_id: &str,
                       event_id: &str,
                       content: &str,
                       context: WorkContext| {
            store.capture_for_account(
                token,
                ObservationInput {
                    context,
                    ..empty_capture(session_id, event_id, content)
                },
            )
        };

        store.start_session_for_account(&owner.token, "handoff-topic", WorkContext::default())?;
        store.start_session_for_account(&member.token, "handoff-topic", WorkContext::default())?;
        capture(
            &mut store,
            &owner.token,
            "handoff-topic",
            "owner-handoff-evt",
            "quotadelta widget does not apply remaining",
            WorkContext::default(),
        )?;
        capture(
            &mut store,
            &member.token,
            "handoff-topic",
            "member-handoff-evt",
            "quotadelta widget does apply foreignmarker remaining",
            WorkContext::default(),
        )?;
        let owner_handoff = store.end_session_for_account(&owner.token, "handoff-topic")?;
        let member_handoff = store.end_session_for_account(&member.token, "handoff-topic")?;
        assert_eq!(owner_handoff.scope, MemoryScope::Session);
        assert_eq!(member_handoff.scope, MemoryScope::Session);
        assert_eq!(owner_handoff.session_id, member_handoff.session_id);

        store.start_session_for_account(&owner.token, "branch-neg", branch.clone())?;
        store.start_session_for_account(&owner.token, "branch-pos", branch.clone())?;
        store.start_session_for_account(&member.token, "branch-neg", branch.clone())?;
        capture(
            &mut store,
            &owner.token,
            "branch-neg",
            "owner-neg-evt",
            "latticebeta signal does not apply remaining",
            branch.clone(),
        )?;
        capture(
            &mut store,
            &owner.token,
            "branch-pos",
            "owner-pos-evt",
            "latticebeta signal does apply remaining",
            branch.clone(),
        )?;
        capture(
            &mut store,
            &member.token,
            "branch-neg",
            "member-branch-evt",
            "latticebeta signal does apply foreignmarker remaining",
            branch.clone(),
        )?;
        let owner_neg = store.end_session_for_account(&owner.token, "branch-neg")?;
        let owner_pos = store.end_session_for_account(&owner.token, "branch-pos")?;
        let member_branch = store.end_session_for_account(&member.token, "branch-neg")?;
        assert_eq!(owner_neg.scope, MemoryScope::Branch);
        assert_eq!(owner_pos.scope, MemoryScope::Branch);

        let hides_foreign = |pack: &ContextPack| {
            let json = serde_json::to_string(pack).expect("pack json");
            assert!(!json.contains(&member.account.person_id), "{json}");
            assert!(!json.contains(&member_handoff.memory_id), "{json}");
            assert!(!json.contains(&member_branch.memory_id), "{json}");
            assert!(!json.contains("member-handoff-evt"), "{json}");
            assert!(!json.contains("member-branch-evt"), "{json}");
            assert!(!json.contains("foreignmarker"), "{json}");
        };

        let resume = store.resume_for_account(&owner.token, 5, &filter)?;
        let handoff = resume
            .results
            .iter()
            .find(|result| result.memory_id == owner_handoff.memory_id)
            .unwrap();
        assert!(!handoff.contradictory);
        assert!(handoff.contradiction_ids.is_empty());
        assert!(handoff.content.contains("does not apply"));
        let pos = resume
            .results
            .iter()
            .find(|result| result.memory_id == owner_pos.memory_id)
            .unwrap();
        let neg = resume
            .results
            .iter()
            .find(|result| result.memory_id == owner_neg.memory_id)
            .unwrap();
        assert!(pos.contradictory);
        assert_eq!(pos.contradiction_ids, vec![owner_neg.memory_id.clone()]);
        assert!(neg.contradictory);
        assert_eq!(neg.contradiction_ids, vec![owner_pos.memory_id.clone()]);
        hides_foreign(&resume);

        let packed = store.resume_for_account(&owner.token, 1, &filter)?;
        assert_eq!(packed.results.len(), 1);
        assert_eq!(packed.results[0].memory_id, owner_pos.memory_id);
        assert_eq!(
            packed.results[0].contradiction_ids,
            vec![owner_neg.memory_id.clone()]
        );
        assert_eq!(
            packed.omitted_memory_ids,
            vec![owner_neg.memory_id.clone(), owner_handoff.memory_id.clone()]
        );
        hides_foreign(&packed);

        let zero = store.resume_for_account(&owner.token, 0, &filter)?;
        assert!(zero.results.is_empty());
        assert_eq!(zero.omitted_count, 3);
        assert_eq!(
            zero.omitted_memory_ids,
            vec![
                owner_pos.memory_id.clone(),
                owner_neg.memory_id.clone(),
                owner_handoff.memory_id.clone()
            ]
        );
        hides_foreign(&zero);

        let unresolved = store.unresolved_for_account(&owner.token, 5, &filter)?;
        let unresolved_handoff = unresolved
            .results
            .iter()
            .find(|result| result.memory_id == owner_handoff.memory_id)
            .unwrap();
        assert!(!unresolved_handoff.contradictory);
        assert!(unresolved_handoff.contradiction_ids.is_empty());
        let unresolved_neg = unresolved
            .results
            .iter()
            .find(|result| result.memory_id == owner_neg.memory_id)
            .unwrap();
        assert!(unresolved_neg.contradictory);
        assert_eq!(
            unresolved_neg.contradiction_ids,
            vec![owner_pos.memory_id.clone()]
        );
        hides_foreign(&unresolved);
        let unresolved_zero = store.unresolved_for_account(&owner.token, 0, &filter)?;
        assert!(unresolved_zero.results.is_empty());
        hides_foreign(&unresolved_zero);

        let hits = store.search_for_account(&owner.token, "quotadelta", 5, &filter)?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].memory_id, owner_handoff.memory_id);
        assert!(!hits[0].contradictory);
        assert!(hits[0].contradiction_ids.is_empty());
        let context = store.context_for_account(&owner.token, "quotadelta", 1, &filter)?;
        assert_eq!(context.results[0].memory_id, owner_handoff.memory_id);
        assert!(context.results[0].contradiction_ids.is_empty());
        hides_foreign(&context);
        let why = store.why_changed_for_account(&owner.token, "quotadelta", 1, &filter)?;
        assert_eq!(why.results[0].memory_id, owner_handoff.memory_id);
        assert!(why.results[0].contradiction_ids.is_empty());
        hides_foreign(&why);

        let unscoped = store.resume_context(20, &filter)?;
        let unscoped_owner = unscoped
            .results
            .iter()
            .find(|result| result.memory_id == owner_handoff.memory_id)
            .unwrap();
        assert!(unscoped_owner.contradictory);
        assert!(
            unscoped_owner
                .contradiction_ids
                .contains(&member_handoff.memory_id)
        );

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn contradiction_targets_follow_recall_eligibility() -> Result<(), StoreError> {
        let path = temp_store_path("contradiction-eligibility");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let member =
            store.register_account("member@example.test", "correct horse battery staple")?;
        let filter = RecallFilter::default();
        let branch = WorkContext {
            repository: Some("cairn-demo".to_owned()),
            branch: Some("main".to_owned()),
            ..WorkContext::default()
        };
        let other_branch = WorkContext {
            repository: Some("cairn-demo".to_owned()),
            branch: Some("topic/other".to_owned()),
            ..WorkContext::default()
        };
        let owned = |store: &mut MemoryStore, content: &str, context: WorkContext| {
            store.remember_for_account(
                &owner.token,
                content,
                Vec::new(),
                context,
                LifecycleInput::default(),
                MemoryKind::Explicit,
            )
        };
        let quiet = |store: &MemoryStore, query: &str| -> Result<(), StoreError> {
            for results in [
                store.search_for_account(&owner.token, query, 5, &filter)?,
                store
                    .context_for_account(&owner.token, query, 5, &filter)?
                    .results,
                store
                    .why_changed_for_account(&owner.token, query, 5, &filter)?
                    .results,
            ] {
                assert_eq!(results.len(), 1, "{query}");
                assert!(!results[0].contradictory, "{query}");
                assert!(results[0].contradiction_ids.is_empty(), "{query}");
            }
            Ok(())
        };

        let neg = owned(
            &mut store,
            "alphaquota widget does not apply",
            branch.clone(),
        )?;
        let pos = owned(&mut store, "alphaquota widget does apply", branch.clone())?;
        assert!(neg.memory_id < pos.memory_id);
        let other = owned(
            &mut store,
            "alphaquota widget does apply",
            other_branch.clone(),
        )?;
        let foreign = store.remember_for_account(
            &member.token,
            "alphaquota widget does apply foreignmarker",
            Vec::new(),
            branch.clone(),
            LifecycleInput::default(),
            MemoryKind::Explicit,
        )?;

        let limited = store.search_for_account(&owner.token, "alphaquota", 1, &filter)?;
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].memory_id, neg.memory_id);
        assert_eq!(limited[0].content, "alphaquota widget does not apply");
        assert!(limited[0].contradictory);
        assert_eq!(limited[0].contradiction_ids, vec![pos.memory_id.clone()]);
        let packed = store.context_for_account(&owner.token, "alphaquota", 1, &filter)?;
        assert_eq!(packed.results[0].memory_id, neg.memory_id);
        assert_eq!(
            packed.results[0].contradiction_ids,
            vec![pos.memory_id.clone()]
        );
        assert!(packed.results[0].contradictory);
        assert_eq!(packed.omitted_count, 2);
        assert!(packed.omitted_memory_ids.contains(&pos.memory_id));
        assert!(packed.omitted_memory_ids.contains(&other.memory_id));
        assert!(!packed.omitted_memory_ids.contains(&foreign.memory_id));
        let why = store.why_changed_for_account(&owner.token, "alphaquota", 5, &filter)?;
        let why_neg = why
            .results
            .iter()
            .find(|result| result.memory_id == neg.memory_id)
            .unwrap();
        let why_pos = why
            .results
            .iter()
            .find(|result| result.memory_id == pos.memory_id)
            .unwrap();
        assert_eq!(why_neg.contradiction_ids, vec![pos.memory_id.clone()]);
        assert_eq!(why_pos.contradiction_ids, vec![neg.memory_id.clone()]);
        assert!(why.results.iter().all(|result| {
            !result.contradiction_ids.contains(&other.memory_id)
                && !result.contradiction_ids.contains(&foreign.memory_id)
                && !result.content.contains("foreignmarker")
        }));
        let unscoped = store.search_with_filter("alphaquota", 10, &filter)?;
        let unscoped_neg = unscoped
            .iter()
            .find(|result| result.memory_id == neg.memory_id)
            .unwrap();
        assert_eq!(
            unscoped_neg.contradiction_ids,
            vec![pos.memory_id.clone(), foreign.memory_id.clone()]
        );
        assert!(!unscoped_neg.contradiction_ids.contains(&other.memory_id));

        let predecessor = owned(
            &mut store,
            "corrquota cache does apply",
            WorkContext::default(),
        )?;
        let peer = owned(
            &mut store,
            "corrquota cache does not apply",
            WorkContext::default(),
        )?;
        let correction = store.correct_for_account(
            &owner.token,
            &predecessor.memory_id,
            "corrquota cache does apply",
            Vec::new(),
            WorkContext::default(),
            LifecycleInput::default(),
        )?;
        assert_eq!(
            correction.supersedes.as_deref(),
            Some(predecessor.memory_id.as_str())
        );
        let while_live = store.search_for_account(&owner.token, "corrquota", 5, &filter)?;
        assert!(
            while_live
                .iter()
                .all(|result| result.memory_id != predecessor.memory_id)
        );
        let live_peer = while_live
            .iter()
            .find(|result| result.memory_id == peer.memory_id)
            .unwrap();
        assert_eq!(
            live_peer.contradiction_ids,
            vec![correction.memory_id.clone()]
        );
        store
            .state
            .memories
            .iter_mut()
            .find(|memory| memory.memory_id == correction.memory_id)
            .unwrap()
            .lifecycle
            .expires_at = Some(now_seconds().saturating_sub(1).to_string());
        assert!(!store.state.forgotten_ids.contains(&correction.memory_id));
        let after_expiry = store.search_for_account(&owner.token, "corrquota", 5, &filter)?;
        assert!(
            after_expiry
                .iter()
                .all(|result| result.memory_id != correction.memory_id)
        );
        let restored = after_expiry
            .iter()
            .find(|result| result.memory_id == predecessor.memory_id)
            .unwrap();
        let restored_peer = after_expiry
            .iter()
            .find(|result| result.memory_id == peer.memory_id)
            .unwrap();
        assert_eq!(restored.contradiction_ids, vec![peer.memory_id.clone()]);
        assert_eq!(
            restored_peer.contradiction_ids,
            vec![predecessor.memory_id.clone()]
        );
        let why_restored = store.why_changed_for_account(&owner.token, "corrquota", 5, &filter)?;
        assert!(why_restored.results.iter().all(|result| {
            !result.contradiction_ids.contains(&correction.memory_id)
                && result.memory_id != correction.memory_id
        }));

        let current = owned(&mut store, "expquota relay does apply", branch.clone())?;
        let expired = store.remember_for_account(
            &owner.token,
            "expquota relay does not apply",
            Vec::new(),
            branch.clone(),
            LifecycleInput {
                expires_at: Some(now_seconds().saturating_sub(1).to_string()),
                ..LifecycleInput::default()
            },
            MemoryKind::Explicit,
        )?;
        assert!(!store.state.forgotten_ids.contains(&expired.memory_id));
        quiet(&store, "expquota")?;
        let unscoped_expired = store.search_with_filter("expquota", 5, &filter)?;
        assert_eq!(unscoped_expired.len(), 1);
        assert_eq!(unscoped_expired[0].memory_id, current.memory_id);
        assert!(unscoped_expired[0].contradiction_ids.is_empty());

        store.start_session_for_account(&owner.token, "sum-live", branch.clone())?;
        store.start_session_for_account(&owner.token, "sum-dead", branch.clone())?;
        store.capture_for_account(
            &owner.token,
            ObservationInput {
                context: branch.clone(),
                ..observation_for(
                    "sum-live",
                    "sum-live-event",
                    "sumquota sumtoken does hold unresolved",
                )
            },
        )?;
        store.capture_for_account(
            &owner.token,
            ObservationInput {
                context: branch.clone(),
                ..observation_for(
                    "sum-dead",
                    "sum-dead-event",
                    "sumquota sumtoken does not hold unresolved",
                )
            },
        )?;
        let live_summary = store.end_session_for_account(&owner.token, "sum-live")?;
        let dead_summary = store.end_session_for_account(&owner.token, "sum-dead")?;
        let live_handoff = store.resume_for_account(&owner.token, 5, &filter)?;
        let live_row = live_handoff
            .results
            .iter()
            .find(|result| result.memory_id == live_summary.memory_id)
            .unwrap();
        assert_eq!(
            live_row.contradiction_ids,
            vec![dead_summary.memory_id.clone()]
        );
        store.forget_for_account(&owner.token, "sum-dead-event")?;
        assert!(store.state.forgotten_ids.contains("sum-dead-event"));
        assert!(!store.state.forgotten_ids.contains(&dead_summary.memory_id));
        quiet(&store, "sumquota")?;
        assert!(
            !store.search_for_account(&owner.token, "sumquota", 5, &filter)?[0]
                .content
                .contains("does not hold")
        );
        for pack in [
            store.resume_for_account(&owner.token, 5, &filter)?,
            store.unresolved_for_account(&owner.token, 5, &filter)?,
        ] {
            assert!(pack.results.iter().all(|result| {
                result.memory_id != dead_summary.memory_id
                    && !result.contradiction_ids.contains(&dead_summary.memory_id)
            }));
            let row = pack
                .results
                .iter()
                .find(|result| result.memory_id == live_summary.memory_id)
                .unwrap();
            assert!(!row.contradictory);
            assert!(row.contradiction_ids.is_empty());
        }

        let kept = owned(&mut store, "idquota beacon does apply", branch.clone())?;
        let forgotten = owned(&mut store, "idquota beacon does not apply", branch.clone())?;
        store.forget_for_account(&owner.token, &forgotten.memory_id)?;
        quiet(&store, "idquota")?;
        assert_eq!(
            store.search_for_account(&owner.token, "idquota", 5, &filter)?[0].memory_id,
            kept.memory_id
        );
        assert!(
            store
                .state
                .memories
                .iter()
                .any(|memory| memory.memory_id == forgotten.memory_id)
        );

        let older = owned(&mut store, "gonequota vault does apply", branch.clone())?;
        let rival = owned(&mut store, "gonequota vault does not apply", branch.clone())?;
        let successor = store.correct_for_account(
            &owner.token,
            &older.memory_id,
            "gonequota vault does apply",
            Vec::new(),
            branch.clone(),
            LifecycleInput::default(),
        )?;
        store.forget_for_account(&owner.token, &successor.memory_id)?;
        let restored_forget = store.search_for_account(&owner.token, "gonequota", 5, &filter)?;
        let older_row = restored_forget
            .iter()
            .find(|result| result.memory_id == older.memory_id)
            .unwrap();
        let rival_row = restored_forget
            .iter()
            .find(|result| result.memory_id == rival.memory_id)
            .unwrap();
        assert_eq!(older_row.contradiction_ids, vec![rival.memory_id.clone()]);
        assert_eq!(rival_row.contradiction_ids, vec![older.memory_id.clone()]);
        assert!(
            restored_forget
                .iter()
                .all(|result| result.memory_id != successor.memory_id)
        );

        let exported = store.export_for_account(&owner.token)?;
        assert!(exported.memories.iter().any(|memory| {
            memory.memory_id == predecessor.memory_id && memory.supersedes.is_none()
        }));
        assert!(exported.memories.iter().any(|memory| {
            memory.memory_id == correction.memory_id
                && memory.supersedes.as_deref() == Some(predecessor.memory_id.as_str())
        }));
        assert!(
            exported
                .memories
                .iter()
                .any(|memory| memory.memory_id == expired.memory_id)
        );
        assert!(
            exported
                .memories
                .iter()
                .any(|memory| memory.memory_id == forgotten.memory_id)
        );
        assert!(
            exported
                .memories
                .iter()
                .any(|memory| memory.memory_id == dead_summary.memory_id)
        );
        assert!(
            exported
                .memories
                .iter()
                .any(|memory| memory.memory_id == successor.memory_id)
        );
        assert!(
            !exported
                .forgotten_ids
                .iter()
                .any(|id| id == &expired.memory_id)
        );

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn forget_reaches_a_later_pull_without_returning_the_observation() -> Result<(), StoreError> {
        let path = temp_store_path("forget-sync");
        let mut store = MemoryStore::open(&path)?;
        let account =
            store.bootstrap_account("forget@example.test", "correct horse battery staple")?;
        let person = account.account.person_id;
        let first = store.register_device("device-a", &person, None, "First")?;
        let second = store.register_device("device-b", &person, None, "Second")?;
        store.sync_push(
            &first.token,
            observation_for("forget-session", "forget-event", "forget me later"),
        )?;
        let pulled = store.sync_pull(&second.token, 0)?;
        assert_eq!(pulled.events.len(), 1);
        assert!(pulled.deletions.is_empty());
        store.forget("forget-event")?;
        let from_start = store.sync_pull(&second.token, 0)?;
        assert!(
            from_start
                .events
                .iter()
                .all(|event| event.event_id != "forget-event")
        );
        let next = store.sync_pull(&second.token, pulled.cursor)?;
        assert!(
            next.events
                .iter()
                .all(|event| event.event_id != "forget-event")
        );
        assert!(
            next.deletions
                .iter()
                .any(|deletion| deletion.item_id == "forget-event")
        );
        let exported = store.export();
        assert!(
            exported
                .observations
                .iter()
                .any(|observation| observation.event_id == "forget-event")
        );
        assert!(exported.forgotten_ids.iter().any(|id| id == "forget-event"));
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn durable_summary_survives_raw_observation_expiry_until_forget() -> Result<(), StoreError> {
        let path = temp_store_path("durable-expiry");
        let mut store = MemoryStore::open(&path)?;
        store.start_session("durable-session", WorkContext::default())?;
        store.capture_observation(ObservationInput {
            event_id: "raw-evt".to_owned(),
            session_id: "durable-session".to_owned(),
            content: "durable summary source evidence".to_owned(),
            role: "agent".to_owned(),
            source: "test".to_owned(),
            importance: None,
            lifecycle: LifecycleInput {
                retention: Some(Retention::Temporary),
                expires_at: Some((now_seconds() + 3600).to_string()),
                ..LifecycleInput::default()
            },
            context: WorkContext::default(),
        })?;
        let summary = store.end_session("durable-session")?;
        assert_eq!(summary.lifecycle.retention, Retention::Durable);
        store
            .state
            .observations
            .iter_mut()
            .find(|observation| observation.event_id == "raw-evt")
            .unwrap()
            .lifecycle
            .expires_at = Some((now_seconds() - 1).to_string());
        store.sweep_expired()?;
        let recalled = store.search("durable summary source evidence", 5)?;
        assert_eq!(recalled.len(), 1);
        assert_eq!(recalled[0].memory_id, summary.memory_id);
        store.forget(&summary.memory_id)?;
        assert!(
            store
                .search("durable summary source evidence", 5)?
                .is_empty()
        );
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn sync_event_id_collision_cannot_leak_another_persons_observation() -> Result<(), StoreError> {
        let path = temp_store_path("device-sync-scope");
        let mut store = MemoryStore::open(&path)?;
        let owner_account =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let other_account =
            store.register_account("other@example.test", "correct horse battery staple")?;
        let owner = store.register_device(
            "device-owner",
            &owner_account.account.person_id,
            None,
            "Owner",
        )?;
        let other = store.register_device(
            "device-other",
            &other_account.account.person_id,
            None,
            "Other",
        )?;

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
    fn capture_for_account_duplicate_stays_with_the_owner() -> Result<(), StoreError> {
        let path = temp_store_path("account-capture-scope");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let member =
            store.register_account("member@example.test", "correct horse battery staple")?;

        let created = store.capture_for_account(
            &owner.token,
            observation_for("sess-evt", "shared-event", "owner private marker"),
        )?;
        assert!(!created.duplicate);
        assert_eq!(created.observation.content, "owner private marker");

        let retry = store.capture_for_account(
            &owner.token,
            observation_for("sess-evt", "shared-event", "owner private marker"),
        )?;
        assert!(retry.duplicate);
        assert_eq!(retry.observation.content, "owner private marker");
        assert_eq!(store.observations().count(), 1);
        assert!(store.observation_owned_by("shared-event", &owner.account.person_id));
        assert!(!store.observation_owned_by("shared-event", &member.account.person_id));

        let foreign = store
            .capture_for_account(
                &member.token,
                observation_for("sess-evt", "shared-event", "member marker"),
            )
            .expect_err("a foreign event id must not return the stored observation");
        let foreign_text = foreign.to_string();
        assert!(
            foreign_text.contains("event belongs to another person: shared-event"),
            "{foreign_text}"
        );
        assert!(!foreign_text.contains("owner private marker"));
        assert!(!foreign_text.contains("member marker"));
        assert!(!foreign_text.contains(&owner.account.person_id));
        assert!(!foreign_text.contains(&member.account.person_id));
        assert_eq!(store.observations().count(), 1);
        assert_eq!(
            store
                .observations()
                .find(|observation| observation.event_id == "shared-event")
                .expect("owned row remains")
                .content,
            "owner private marker"
        );
        assert!(store.observation_owned_by("shared-event", &owner.account.person_id));
        assert!(!store.observation_owned_by("shared-event", &member.account.person_id));
        assert_eq!(
            store
                .state
                .observation_owners
                .iter()
                .filter(|record| record.event_id == "shared-event")
                .count(),
            1
        );

        store.capture_observation(observation_for(
            "sess-anon",
            "anon-event",
            "anonymous marker",
        ))?;
        let owners_before_claim = store.state.observation_owners.len();
        let unowned = store
            .capture_for_account(
                &owner.token,
                observation_for("sess-anon", "anon-event", "claim marker"),
            )
            .expect_err("an unowned event must not be claimed or returned");
        let unowned_text = unowned.to_string();
        assert!(
            unowned_text.contains("event already exists outside sync scope: anon-event"),
            "{unowned_text}"
        );
        assert!(!unowned_text.contains("anonymous marker"));
        assert!(!unowned_text.contains("claim marker"));
        assert!(!unowned_text.contains(&owner.account.person_id));
        assert_eq!(store.observations().count(), 2);
        assert_eq!(
            store
                .observations()
                .find(|observation| observation.event_id == "anon-event")
                .expect("anonymous row remains")
                .content,
            "anonymous marker"
        );
        assert!(!store.observation_owned_by("anon-event", &owner.account.person_id));
        assert!(!store.observation_owned_by("anon-event", &member.account.person_id));
        assert_eq!(store.state.observation_owners.len(), owners_before_claim);
        assert!(
            !store
                .state
                .observation_owners
                .iter()
                .any(|record| record.event_id == "anon-event")
        );

        let unauthenticated = store
            .capture_for_account(
                "not-a-session-token",
                observation_for("sess-evt", "shared-event", "owner private marker"),
            )
            .expect_err("authentication is required before ownership is examined");
        let unauthenticated_text = unauthenticated.to_string();
        assert!(unauthenticated_text.contains("account authentication failed"));
        assert!(!unauthenticated_text.contains("owner private marker"));
        assert!(!unauthenticated_text.contains("event belongs to another person"));
        assert_eq!(store.observations().count(), 2);
        assert_eq!(store.state.observation_owners.len(), owners_before_claim);

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

    // The PostgreSQL read queries cast `lifecycle->>'expiresAt'` with `::bigint`, so every
    // write path must accept exactly the values that cast accepts: 0..=i64::MAX.
    #[test]
    fn expiry_is_bounded_to_i64_unix_seconds_on_input_and_import_paths() -> Result<(), StoreError> {
        let path = temp_store_path("expiry-bound");
        let mut source = MemoryStore::open(&path)?;
        source.capture_observation(observation("expiry-bound-event", "expiryfact"))?;
        source.remember("expiryfact memory", Vec::new(), context())?;
        let bundle = source.export();
        fs::remove_file(path)?;

        let max = i64::MAX.to_string();
        for (value, valid) in [
            (max.as_str(), true),
            ("0009223372036854775807", true),
            ("0", true),
            ("+5", true),
            ("0000000005", true),
            ("9223372036854775808", false),
            ("0009223372036854775808", false),
            ("18446744073709551615", false),
            ("18446744073709551616", false),
            ("-1", false),
            ("", false),
            ("soon", false),
        ] {
            let lifecycle = LifecycleInput {
                expires_at: Some(value.to_owned()),
                ..LifecycleInput::default()
            };
            let mut input = observation("expiry-bound-input", "expiryfact");
            input.lifecycle = lifecycle.clone();
            let remembered = build_explicit_memory_record(
                "expiryfact",
                Vec::new(),
                WorkContext::default(),
                lifecycle,
                MemoryKind::Explicit,
            );
            let mut imported = bundle.clone();
            imported.observations[0].lifecycle.expires_at = Some(value.to_owned());
            imported.memories[0].lifecycle.expires_at = Some(value.to_owned());
            assert_eq!(
                validate_observation(&input).is_ok(),
                valid,
                "observation {value:?}"
            );
            assert_eq!(remembered.is_ok(), valid, "remember {value:?}");
            assert_eq!(
                normalize_export_bundle(&mut imported).is_ok(),
                valid,
                "import {value:?}"
            );
        }
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
    fn redacts_private_key_and_mixed_case_sk_and_keeps_sensitivity_floor() -> Result<(), StoreError>
    {
        const BODY: &str = "c3ludGhldGljLWtleS1ib2R5";
        const SK: &str = "SK-synthetic-example";
        struct Case {
            content: &'static str,
            supplied: Option<Sensitivity>,
            sensitivity: Sensitivity,
            absent: &'static [&'static str],
            present: &'static [&'static str],
        }
        let cases = [
            Case {
                content: "prefix SK-synthetic-example suffix",
                supplied: Some(Sensitivity::Public),
                sensitivity: Sensitivity::Secret,
                absent: &[SK],
                present: &["prefix ", " suffix", REDACTED],
            },
            Case {
                content: "sk-synthetic-example",
                supplied: None,
                sensitivity: Sensitivity::Secret,
                absent: &["sk-synthetic-example"],
                present: &[REDACTED],
            },
            Case {
                content: "Sk-synthetic-example",
                supplied: Some(Sensitivity::Sensitive),
                sensitivity: Sensitivity::Secret,
                absent: &["Sk-synthetic-example"],
                present: &[REDACTED],
            },
            Case {
                content: "disk-full, risk-based task-queue",
                supplied: None,
                sensitivity: Sensitivity::Private,
                absent: &[REDACTED],
                present: &["disk-full, risk-based task-queue"],
            },
            Case {
                content: "key=(sk-synthetic-example) and \"SK-synthetic-example\"",
                supplied: None,
                sensitivity: Sensitivity::Secret,
                absent: &["sk-synthetic-example", "SK-synthetic-example"],
                present: &["key=(", " and \"", REDACTED],
            },
            Case {
                content: "before\r\n-----BEGIN RSA PRIVATE KEY-----\r\nc3ludGhldGljLWtleS1ib2R5\r\n-----END RSA PRIVATE KEY-----\r\nafter",
                supplied: Some(Sensitivity::Public),
                sensitivity: Sensitivity::Secret,
                absent: &[BODY, "BEGIN RSA PRIVATE KEY"],
                present: &["before", "after", REDACTED],
            },
            Case {
                content: "-----Begin Ec Private Key-----\nc3ludGhldGljLWtleS1ib2R5\n-----End Ec Private Key-----\n",
                supplied: None,
                sensitivity: Sensitivity::Secret,
                absent: &[BODY, "Ec Private Key"],
                present: &[REDACTED],
            },
            Case {
                content: "notes before\n-----BEGIN OPENSSH PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5\ntail note stays\n",
                supplied: None,
                sensitivity: Sensitivity::Secret,
                absent: &[BODY, "BEGIN OPENSSH PRIVATE KEY"],
                present: &["notes before", "tail note stays"],
            },
            Case {
                content: "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,SYNTHETICIVVALUE\nc3ludGhldGljLWtleS1ib2R5\nordinary sentence stays\n",
                supplied: None,
                sensitivity: Sensitivity::Secret,
                absent: &[BODY, "SYNTHETICIVVALUE", "Proc-Type"],
                present: &["ordinary sentence stays"],
            },
            Case {
                content: "-----BEGIN ENCRYPTED PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5\n-----END ENCRYPTED PRIVATE KEY-----\n",
                supplied: None,
                sensitivity: Sensitivity::Secret,
                absent: &[BODY, "ENCRYPTED PRIVATE KEY"],
                present: &[REDACTED],
            },
            Case {
                content: "-----BEGIN PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5\n-----END PRIVATE KEY-----\n",
                supplied: None,
                sensitivity: Sensitivity::Secret,
                absent: &[BODY, "BEGIN PRIVATE KEY"],
                present: &[REDACTED],
            },
            Case {
                content: "lead -----BEGIN EC PRIVATE KEY----- c3ludGhldGljLWtleS1ib2R5",
                supplied: None,
                sensitivity: Sensitivity::Secret,
                absent: &[BODY, "BEGIN EC PRIVATE KEY"],
                present: &["lead ", REDACTED],
            },
            Case {
                content: "-----BEGIN RSA PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5-----END RSA PRIVATE KEY\nnext prose stays\n",
                supplied: Some(Sensitivity::Public),
                sensitivity: Sensitivity::Secret,
                absent: &[BODY, "BEGIN RSA PRIVATE KEY", "END RSA PRIVATE KEY"],
                present: &["next prose stays", REDACTED],
            },
            Case {
                content: "-----BEGIN RSA PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5-----END RSA PRIVATE KEY then the note stays\n",
                supplied: None,
                sensitivity: Sensitivity::Secret,
                absent: &[BODY, "END RSA PRIVATE KEY"],
                present: &["then the note stays", REDACTED],
            },
            Case {
                content: "-----Begin Rsa Private Key-----\r\nc3ludGhldGljLWtleS1ib2R5-----End Rsa Private Key\r\nafter glue\r\n",
                supplied: None,
                sensitivity: Sensitivity::Secret,
                absent: &[BODY, "End Rsa Private Key"],
                present: &["after glue", REDACTED],
            },
            Case {
                content: "-----BEGIN EC PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5-----END EC PRIVATE KEY-----\nkept complete\n",
                supplied: None,
                sensitivity: Sensitivity::Secret,
                absent: &[BODY],
                present: &["kept complete", REDACTED],
            },
            Case {
                content: "-----BEGIN RSA PRIVATE KEY-----\nThe armor label is PRIVATE KEY in prose.\n",
                supplied: Some(Sensitivity::Public),
                sensitivity: Sensitivity::Secret,
                absent: &[BODY],
                present: &["The armor label is PRIVATE KEY in prose.", REDACTED],
            },
            Case {
                content: "-----BEGIN RSA PRIVATE KEY\nc3ludGhldGljLWtleS1ib2R5\n",
                supplied: None,
                sensitivity: Sensitivity::Sensitive,
                absent: &[REDACTED],
                present: &[BODY, "-----BEGIN RSA PRIVATE KEY"],
            },
            Case {
                content: "-----BEGIN DSA PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5\n-----END DSA PRIVATE KEY-----\n",
                supplied: None,
                sensitivity: Sensitivity::Sensitive,
                absent: &[REDACTED],
                present: &[BODY, "BEGIN DSA PRIVATE KEY"],
            },
            Case {
                content: "-----BEGIN CERTIFICATE-----\nbm90LWEtY2VydA==\n-----END CERTIFICATE-----\n",
                supplied: None,
                sensitivity: Sensitivity::Private,
                absent: &[REDACTED],
                present: &[
                    "-----BEGIN CERTIFICATE-----\nbm90LWEtY2VydA==\n-----END CERTIFICATE-----\n",
                ],
            },
            Case {
                content: "-----BEGIN PUBLIC KEY-----\nbm90LWEtcHVibGljLWtleQ==\n-----END PUBLIC KEY-----\n",
                supplied: None,
                sensitivity: Sensitivity::Private,
                absent: &[REDACTED],
                present: &[
                    "-----BEGIN PUBLIC KEY-----\nbm90LWEtcHVibGljLWtleQ==\n-----END PUBLIC KEY-----\n",
                ],
            },
            Case {
                content: "-----BEGIN CERTIFICATE-----\nstill visible tail\n",
                supplied: None,
                sensitivity: Sensitivity::Private,
                absent: &[REDACTED],
                present: &["-----BEGIN CERTIFICATE-----", "still visible tail"],
            },
            Case {
                content: "The armor label is PRIVATE KEY in prose.",
                supplied: Some(Sensitivity::Public),
                sensitivity: Sensitivity::Sensitive,
                absent: &[REDACTED],
                present: &["The armor label is PRIVATE KEY in prose."],
            },
            Case {
                content: "caf\u{e9} menu SK-synthetic-example \u{43a}\u{43e}\u{43d}\u{435}\u{446}",
                supplied: None,
                sensitivity: Sensitivity::Secret,
                absent: &[SK],
                present: &[
                    "caf\u{e9} menu ",
                    " \u{43a}\u{43e}\u{43d}\u{435}\u{446}",
                    REDACTED,
                ],
            },
            Case {
                content: "line\r\nSK-synthetic-example\r\nnext",
                supplied: None,
                sensitivity: Sensitivity::Secret,
                absent: &[SK],
                present: &["line", "next"],
            },
            Case {
                content: "Authorization: Bearer synthetic-bearer-value api_key=synthetic-api-value token=synthetic-token-value password=synthetic-pass-value",
                supplied: None,
                sensitivity: Sensitivity::Secret,
                absent: &[
                    "synthetic-bearer-value",
                    "synthetic-api-value",
                    "synthetic-token-value",
                    "synthetic-pass-value",
                ],
                present: &[
                    "Authorization: Bearer [REDACTED]",
                    "api_key=[REDACTED]",
                    "token=[REDACTED]",
                    "password=[REDACTED]",
                ],
            },
            Case {
                content: "Password=synthetic-pass-value",
                supplied: None,
                sensitivity: Sensitivity::Secret,
                absent: &["synthetic-pass-value"],
                present: &["Password=[REDACTED]"],
            },
            Case {
                content: "The design review is on Thursday.",
                supplied: Some(Sensitivity::Secret),
                sensitivity: Sensitivity::Secret,
                absent: &[REDACTED],
                present: &["The design review is on Thursday."],
            },
            Case {
                content: "The design review is on Thursday.",
                supplied: Some(Sensitivity::Sensitive),
                sensitivity: Sensitivity::Sensitive,
                absent: &[REDACTED],
                present: &["The design review is on Thursday."],
            },
            Case {
                content: "The design review is on Thursday.",
                supplied: Some(Sensitivity::Public),
                sensitivity: Sensitivity::Private,
                absent: &[REDACTED],
                present: &["The design review is on Thursday."],
            },
            Case {
                content: "confidential roadmap note",
                supplied: Some(Sensitivity::Secret),
                sensitivity: Sensitivity::Secret,
                absent: &[REDACTED],
                present: &["confidential roadmap note"],
            },
            Case {
                content: "confidential roadmap note",
                supplied: Some(Sensitivity::Public),
                sensitivity: Sensitivity::Sensitive,
                absent: &[REDACTED],
                present: &["confidential roadmap note"],
            },
        ];

        for (index, case) in cases.iter().enumerate() {
            let mut input = observation(&format!("evt-redact-{index}"), case.content);
            input.lifecycle.sensitivity = case.supplied.clone();
            let observation = normalize_observation(&input)?;
            let memory = build_explicit_memory_record(
                case.content,
                Vec::new(),
                WorkContext::default(),
                LifecycleInput {
                    sensitivity: case.supplied.clone(),
                    ..LifecycleInput::default()
                },
                MemoryKind::Explicit,
            )?;
            for absent in case.absent {
                assert!(
                    !observation.content.contains(absent),
                    "case {index} observation still has {absent}: {}",
                    observation.content
                );
                assert!(
                    !memory.content.contains(absent),
                    "case {index} memory still has {absent}: {}",
                    memory.content
                );
            }
            for present in case.present {
                assert!(
                    observation.content.contains(present),
                    "case {index} observation missing {present}: {}",
                    observation.content
                );
                assert!(
                    memory.content.contains(present),
                    "case {index} memory missing {present}: {}",
                    memory.content
                );
            }
            assert_eq!(
                observation.lifecycle.sensitivity, case.sensitivity,
                "case {index} observation sensitivity"
            );
            assert_eq!(
                memory.lifecycle.sensitivity, case.sensitivity,
                "case {index} memory sensitivity"
            );
        }

        let mut certificates = "-----BEGIN CERTIFICATE-----\n".repeat(2048);
        certificates.push_str("caf\u{e9} stays");
        let kept = normalize_observation(&observation("evt-redact-certs", &certificates))?;
        assert!(kept.content.contains("-----BEGIN CERTIFICATE-----"));
        assert!(kept.content.contains("caf\u{e9} stays"));
        assert!(!kept.content.contains(REDACTED));
        assert_eq!(kept.lifecycle.sensitivity, Sensitivity::Private);

        let path = temp_store_path("redaction-floor");
        let mut store = MemoryStore::open(&path)?;
        let captured = store.capture_observation(observation_with_sensitivity(
            "evt-redact-stored",
            "caf\u{e9} SK-synthetic-example\n-----BEGIN OPENSSH PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5\n\u{43a}\u{43e}\u{43d}\u{435}\u{446}",
            Some(Sensitivity::Public),
        ))?;
        let remembered = store.remember_with_lifecycle(
            "caf\u{e9} SK-synthetic-example\n-----BEGIN PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5\n-----END PRIVATE KEY-----\n",
            vec!["SK-synthetic-example".to_owned()],
            context(),
            LifecycleInput {
                sensitivity: Some(Sensitivity::Public),
                ..LifecycleInput::default()
            },
        )?;
        let glued = store.capture_observation(observation_with_sensitivity(
            "evt-redact-glued-footer",
            "-----BEGIN RSA PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5-----END RSA PRIVATE KEY then the note stays\n",
            Some(Sensitivity::Public),
        ))?;
        assert!(
            !glued.observation.content.contains(BODY),
            "glued footer stored {}",
            glued.observation.content
        );
        assert!(glued.observation.content.contains("then the note stays"));
        assert!(!glued.observation.content.contains("END RSA PRIVATE KEY"));
        assert_eq!(glued.observation.lifecycle.sensitivity, Sensitivity::Secret);
        let glued_json = serde_json::to_string(&glued.observation).expect("glued json");
        assert!(!glued_json.contains(BODY));

        let stricter = store.remember_with_lifecycle(
            "The design review is on Thursday.",
            Vec::new(),
            context(),
            LifecycleInput {
                sensitivity: Some(Sensitivity::Secret),
                ..LifecycleInput::default()
            },
        )?;
        let captured_json = serde_json::to_string(&captured.observation).expect("observation json");
        let remembered_json = serde_json::to_string(&remembered).expect("memory json");
        let persisted = fs::read_to_string(&path)?;
        for secret in [SK, BODY] {
            assert!(!captured.observation.content.contains(secret));
            assert!(!remembered.content.contains(secret));
            assert!(!captured_json.contains(secret));
            assert!(!remembered_json.contains(secret));
            assert!(!persisted.contains(secret));
        }
        assert!(captured.observation.content.contains("caf\u{e9} "));
        assert!(
            captured
                .observation
                .content
                .contains("\u{43a}\u{43e}\u{43d}\u{435}\u{446}")
        );
        assert_eq!(
            captured.observation.lifecycle.sensitivity,
            Sensitivity::Secret
        );
        assert_eq!(remembered.lifecycle.sensitivity, Sensitivity::Secret);
        assert_eq!(stricter.lifecycle.sensitivity, Sensitivity::Secret);
        assert_eq!(stricter.content, "The design review is on Thursday.");
        assert!(persisted.contains("The design review is on Thursday."));
        assert!(persisted.contains("then the note stays"));
        assert!(!persisted.contains("END RSA PRIVATE KEY"));

        fs::remove_file(path)?;
        Ok(())
    }

    fn observation_with_sensitivity(
        event_id: &str,
        content: &str,
        sensitivity: Option<Sensitivity>,
    ) -> ObservationInput {
        let mut input = observation(event_id, content);
        input.lifecycle.sensitivity = sensitivity;
        input
    }

    fn metadata_secret_context() -> WorkContext {
        WorkContext {
            project_id: None,
            repository: Some("cairn-demo".to_owned()),
            branch: Some("sk-looking-name".to_owned()),
            worktree: Some("wt-auth".to_owned()),
            commit: Some("abc1234".to_owned()),
            files: vec!["src/lib.rs".to_owned()],
            symbols: vec!["MemoryStore::open".to_owned()],
            commands: vec!["echo café SK-synthetic-example".to_owned()],
            tests: vec!["redacts_human_metadata".to_owned()],
            services: vec!["cairn".to_owned()],
            handoffs: vec!["handoff stays".to_owned()],
            runbooks: vec![
                "-----BEGIN RSA PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5\n-----END RSA PRIVATE KEY-----\n"
                    .to_owned(),
            ],
        }
    }

    fn assert_metadata_redacted(context: &WorkContext) {
        const SK: &str = "SK-synthetic-example";
        const BODY: &str = "c3ludGhldGljLWtleS1ib2R5";
        let rendered = serde_json::to_string(context).expect("context json");
        assert!(!rendered.contains(SK), "context still has {SK}: {rendered}");
        assert!(
            !rendered.contains(BODY),
            "context still has {BODY}: {rendered}"
        );
        assert!(!rendered.contains("BEGIN RSA PRIVATE KEY"), "{rendered}");
        assert_eq!(context.repository.as_deref(), Some("cairn-demo"));
        assert_eq!(context.branch.as_deref(), Some("sk-looking-name"));
        assert_eq!(context.worktree.as_deref(), Some("wt-auth"));
        assert_eq!(context.commit.as_deref(), Some("abc1234"));
        assert_eq!(context.files, vec!["src/lib.rs".to_owned()]);
        assert_eq!(context.symbols, vec!["MemoryStore::open".to_owned()]);
        assert_eq!(context.commands, vec!["echo café [REDACTED]".to_owned()]);
        assert_eq!(context.tests, vec!["redacts_human_metadata".to_owned()]);
        assert_eq!(context.services, vec!["cairn".to_owned()]);
        assert_eq!(context.handoffs, vec!["handoff stays".to_owned()]);
        assert!(
            context
                .runbooks
                .iter()
                .all(|value| value.contains(REDACTED))
        );
        assert!(context.commands[0].contains("café"));
    }

    #[test]
    fn redacts_human_metadata_before_new_persistence() -> Result<(), StoreError> {
        const SK: &str = "SK-synthetic-example";
        const BODY: &str = "c3ludGhldGljLWtleS1ib2R5";
        let secret_context = metadata_secret_context();
        let mut captured_input = observation_with_sensitivity(
            "evt-meta-capture",
            "Ordinary capture note. next step: check the café log",
            Some(Sensitivity::Public),
        );
        captured_input.session_id = "sess-meta".to_owned();
        captured_input.context = secret_context.clone();
        let normalized = normalize_observation(&captured_input)?;
        assert_metadata_redacted(&normalized.context);
        assert_eq!(normalized.event_id, "evt-meta-capture");
        assert_eq!(normalized.lifecycle.sensitivity, Sensitivity::Secret);
        assert!(!normalized.content.contains(SK));
        assert!(normalized.content.contains("café"));

        let remembered = build_explicit_memory_record(
            "Ordinary explicit note.",
            vec![SK.to_owned()],
            secret_context.clone(),
            LifecycleInput {
                sensitivity: Some(Sensitivity::Public),
                ..LifecycleInput::default()
            },
            MemoryKind::Explicit,
        )?;
        assert_metadata_redacted(&remembered.context);
        assert_eq!(remembered.lifecycle.sensitivity, Sensitivity::Secret);
        assert!(!remembered.content.contains(SK));
        assert!(remembered.content.contains("Concepts: [REDACTED]"));
        let stricter = build_explicit_memory_record(
            "Ordinary explicit note.",
            Vec::new(),
            context(),
            LifecycleInput {
                sensitivity: Some(Sensitivity::Secret),
                ..LifecycleInput::default()
            },
            MemoryKind::Explicit,
        )?;
        assert_eq!(stricter.lifecycle.sensitivity, Sensitivity::Secret);
        assert_eq!(stricter.context.commands, context().commands);

        let raw_observation = Observation {
            event_id: "evt-derive-old".to_owned(),
            session_id: "sess-derive".to_owned(),
            content: "ordinary derive source".to_owned(),
            role: "assistant".to_owned(),
            source: "test".to_owned(),
            importance: None,
            lifecycle: Lifecycle::default(),
            scope: MemoryScope::Session,
            context: secret_context.clone(),
            captured_at: "1".to_owned(),
        };
        let derived = derive_session_memory(
            None,
            "sess-derive",
            &WorkContext::default(),
            &[raw_observation],
        )?;
        assert_eq!(
            derived.context.commands,
            vec!["echo café [REDACTED]".to_owned()]
        );
        assert_eq!(derived.context.branch.as_deref(), Some("sk-looking-name"));
        assert!(derived.context.handoffs.is_empty());
        assert_eq!(derived.source_event_ids, vec!["evt-derive-old".to_owned()]);
        assert_eq!(derived.lifecycle.sensitivity, Sensitivity::Private);
        assert!(!derived.content.contains(SK));
        assert!(!derived.content.contains(BODY));

        let path = temp_store_path("metadata-redaction");
        let mut store = MemoryStore::open(&path)?;
        let started = store.start_session("sess-meta", secret_context.clone())?;
        assert_metadata_redacted(&started.context);
        let reopened_start = store.start_session(
            "sess-meta",
            WorkContext {
                commands: vec![SK.to_owned()],
                ..WorkContext::default()
            },
        )?;
        assert_eq!(reopened_start.context, started.context);
        assert_eq!(reopened_start.session_id, "sess-meta");
        let inherited = store.capture_observation(ObservationInput {
            event_id: "evt-meta-inherited".to_owned(),
            session_id: "sess-meta".to_owned(),
            content: "Inherited session context is already stored.".to_owned(),
            role: "assistant".to_owned(),
            source: "test".to_owned(),
            importance: None,
            lifecycle: LifecycleInput {
                sensitivity: Some(Sensitivity::Public),
                ..LifecycleInput::default()
            },
            context: WorkContext::default(),
        })?;
        assert_eq!(inherited.observation.context, started.context);
        assert_eq!(
            inherited.observation.lifecycle.sensitivity,
            Sensitivity::Private
        );
        assert_eq!(inherited.observation.event_id, "evt-meta-inherited");
        let captured = store.capture_observation(captured_input)?;
        assert_metadata_redacted(&captured.observation.context);
        assert_eq!(
            captured.observation.lifecycle.sensitivity,
            Sensitivity::Secret
        );
        let partial = store.capture_observation(ObservationInput {
            event_id: "evt-meta-partial".to_owned(),
            session_id: "sess-meta".to_owned(),
            content: "A partial context does not inherit the session branch.".to_owned(),
            role: "assistant".to_owned(),
            source: "test".to_owned(),
            importance: None,
            lifecycle: LifecycleInput::default(),
            context: WorkContext {
                commands: vec![SK.to_owned()],
                ..WorkContext::default()
            },
        })?;
        assert!(partial.observation.context.branch.is_none());
        assert_eq!(
            partial.observation.context.commands,
            vec!["[REDACTED]".to_owned()]
        );
        assert_eq!(
            partial.observation.lifecycle.sensitivity,
            Sensitivity::Secret
        );
        let summary = store.end_session("sess-meta")?;
        assert_eq!(summary.lifecycle.sensitivity, Sensitivity::Secret);
        assert_eq!(summary.session_id, "sess-meta");
        assert!(
            summary
                .source_event_ids
                .contains(&"evt-meta-capture".to_owned())
        );
        assert!(!summary.content.contains(SK));
        assert!(summary.next_steps.iter().all(|step| !step.contains(SK)));
        let target = store.remember(
            "Ordinary correction target.",
            Vec::new(),
            WorkContext {
                repository: Some("cairn-demo".to_owned()),
                ..WorkContext::default()
            },
        )?;
        let corrected = store.correct(
            &target.memory_id,
            "Ordinary correction note.",
            vec!["token=synthetic-concept".to_owned()],
            secret_context,
            LifecycleInput {
                sensitivity: Some(Sensitivity::Public),
                ..LifecycleInput::default()
            },
        )?;
        assert_metadata_redacted(&corrected.context);
        assert_eq!(corrected.lifecycle.sensitivity, Sensitivity::Secret);
        assert_eq!(
            corrected.supersedes.as_deref(),
            Some(target.memory_id.as_str())
        );
        assert!(!corrected.content.contains("synthetic-concept"));
        assert!(!corrected.content.contains(SK));
        let persisted = fs::read_to_string(&path)?;
        let exported = serde_json::to_string(&store.export())?;
        for secret in [SK, BODY, "BEGIN RSA PRIVATE KEY", "synthetic-concept"] {
            assert!(!persisted.contains(secret), "store still has {secret}");
            assert!(!exported.contains(secret), "export still has {secret}");
        }
        assert!(persisted.contains("sk-looking-name"));
        assert!(persisted.contains("evt-meta-capture"));
        assert!(persisted.contains("café"));
        drop(store);
        let mut reopened = MemoryStore::open(&path)?;
        let session = reopened
            .sessions()
            .find(|session| session.session_id == "sess-meta")
            .expect("session reopened");
        assert_metadata_redacted(&session.context);
        assert!(!fs::read_to_string(&path)?.contains(SK));

        let bundle = ExportBundle {
            format: EXPORT_FORMAT.to_owned(),
            version: EXPORT_VERSION,
            exported_at: "10".to_owned(),
            sessions: vec![Session {
                session_id: "sess-import-meta".to_owned(),
                person_id: None,
                context: metadata_secret_context(),
                started_at: "10".to_owned(),
                ended_at: None,
            }],
            observations: vec![Observation {
                event_id: "evt-import-meta".to_owned(),
                session_id: "sess-import-meta".to_owned(),
                content: "ordinary import observation".to_owned(),
                role: "assistant".to_owned(),
                source: "test".to_owned(),
                importance: None,
                lifecycle: Lifecycle {
                    sensitivity: Sensitivity::Public,
                    ..Lifecycle::default()
                },
                scope: MemoryScope::Session,
                context: metadata_secret_context(),
                captured_at: "10".to_owned(),
            }],
            memories: vec![Memory {
                memory_id: "memory-import-meta".to_owned(),
                session_id: "sess-import-meta".to_owned(),
                content: "ordinary import memory".to_owned(),
                source_event_ids: vec!["evt-import-meta".to_owned()],
                context: metadata_secret_context(),
                lifecycle: Lifecycle {
                    sensitivity: Sensitivity::Public,
                    ..Lifecycle::default()
                },
                kind: MemoryKind::Explicit,
                scope: MemoryScope::Global,
                project_id: None,
                next_steps: vec!["next step SK-synthetic-example".to_owned()],
                unresolved_work: vec![
                    "-----BEGIN RSA PRIVATE KEY-----\nc3ludGhldGljLWtleS1ib2R5\n-----END RSA PRIVATE KEY-----\n"
                        .to_owned(),
                ],
                supersedes: None,
                promoted_from: Some("memory-source-1".to_owned()),
                promotion_reason: Some("because SK-synthetic-example".to_owned()),
                verified_commit: Some("abc1234".to_owned()),
                created_at: "10".to_owned(),
            }],
            forgotten_ids: vec![],
        };
        let imported = reopened.import_bundle(bundle.clone())?;
        assert_eq!(imported.imported_sessions, 1);
        assert_eq!(imported.imported_observations, 1);
        assert_eq!(imported.imported_memories, 1);
        let again = reopened.import_bundle(bundle)?;
        assert_eq!(again.imported_memories, 0);
        let import_file = fs::read_to_string(&path)?;
        let import_export = serde_json::to_string(&reopened.export())?;
        for secret in [SK, BODY, "BEGIN RSA PRIVATE KEY"] {
            assert!(
                !import_file.contains(secret),
                "import store still has {secret}"
            );
            assert!(
                !import_export.contains(secret),
                "import export still has {secret}"
            );
        }
        let imported_memory = reopened
            .export()
            .memories
            .into_iter()
            .find(|memory| memory.memory_id == "memory-import-meta")
            .expect("imported memory");
        assert_eq!(imported_memory.lifecycle.sensitivity, Sensitivity::Public);
        assert_eq!(
            imported_memory.promoted_from.as_deref(),
            Some("memory-source-1")
        );
        assert_eq!(imported_memory.verified_commit.as_deref(), Some("abc1234"));
        assert_eq!(
            imported_memory.source_event_ids,
            vec!["evt-import-meta".to_owned()]
        );
        assert_metadata_redacted(&imported_memory.context);
        assert!(
            imported_memory
                .next_steps
                .iter()
                .all(|step| !step.contains(SK))
        );
        assert!(
            imported_memory
                .next_steps
                .iter()
                .any(|step| step.contains(REDACTED))
        );
        assert!(
            imported_memory
                .unresolved_work
                .iter()
                .all(|item| !item.contains(BODY))
        );
        assert_eq!(
            imported_memory.promotion_reason.as_deref(),
            Some("because [REDACTED]")
        );
        let imported_session = reopened
            .sessions()
            .find(|session| session.session_id == "sess-import-meta")
            .expect("imported session");
        assert_metadata_redacted(&imported_session.context);
        fs::remove_file(&path)?;

        let old_path = temp_store_path("metadata-old");
        let old_store = serde_json::json!({
            "version": 1,
            "observations": [{
                "event_id": "evt-old-meta",
                "session_id": "sess-old-meta",
                "content": "ordinary old note",
                "role": "assistant",
                "source": "test",
                "importance": null,
                "context": {"commands": [SK], "branch": "feat/auth"},
                "captured_at": "1"
            }],
            "memories": [{
                "memory_id": "memory-old-meta",
                "session_id": "sess-old-meta",
                "content": "ordinary old memory",
                "source_event_ids": ["evt-old-meta"],
                "context": {"commands": [SK], "branch": "feat/auth"},
                "created_at": "1"
            }]
        });
        fs::write(&old_path, serde_json::to_string(&old_store)?)?;
        let before = fs::read_to_string(&old_path)?;
        let mut old = MemoryStore::open(&old_path)?;
        assert_eq!(fs::read_to_string(&old_path)?, before);
        let old_observation = old
            .observations()
            .find(|observation| observation.event_id == "evt-old-meta")
            .expect("old observation");
        assert_eq!(old_observation.context.commands, vec![SK.to_owned()]);
        assert_eq!(old_observation.context.branch.as_deref(), Some("feat/auth"));
        let inherited_correction = old.correct(
            "memory-old-meta",
            "Ordinary correction of an old memory.",
            Vec::new(),
            WorkContext::default(),
            LifecycleInput {
                sensitivity: Some(Sensitivity::Public),
                ..LifecycleInput::default()
            },
        )?;
        assert_eq!(
            inherited_correction.context.commands,
            vec!["[REDACTED]".to_owned()]
        );
        assert_eq!(
            inherited_correction.lifecycle.sensitivity,
            Sensitivity::Secret
        );
        assert_eq!(
            inherited_correction.supersedes.as_deref(),
            Some("memory-old-meta")
        );
        assert_eq!(
            inherited_correction.context.branch.as_deref(),
            Some("feat/auth")
        );
        let still_old = old
            .export()
            .memories
            .into_iter()
            .find(|memory| memory.memory_id == "memory-old-meta")
            .expect("old memory");
        assert_eq!(still_old.context.commands, vec![SK.to_owned()]);
        assert_eq!(still_old.lifecycle.sensitivity, Sensitivity::Private);
        let old_file = fs::read_to_string(&old_path)?;
        assert!(old_file.contains(SK));
        let new_only = serde_json::to_string(&inherited_correction)?;
        assert!(!new_only.contains(SK));
        assert!(new_only.contains("memory-old-meta"));
        fs::remove_file(old_path)?;
        Ok(())
    }

    #[test]
    fn a_replay_with_changed_content_or_outside_sync_is_a_conflict() -> Result<(), StoreError> {
        let path = temp_store_path("changed-replay");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let device =
            store.register_device("device-a", owner.account.person_id.clone(), None, "MacBook")?;
        let original = observation_for("sess-a", "evt-a", "original content");
        store.capture_for_account(&owner.token, original.clone())?;
        let pushed = store.sync_push(&device.token, original.clone())?;
        let persisted = fs::read(&path)?;

        let changes = [
            ObservationInput {
                content: "changed content".to_owned(),
                ..original.clone()
            },
            ObservationInput {
                session_id: "sess-b".to_owned(),
                ..original.clone()
            },
            ObservationInput {
                role: "user".to_owned(),
                ..original.clone()
            },
            ObservationInput {
                source: "other".to_owned(),
                ..original.clone()
            },
        ];
        for changed in changes {
            let account = store.capture_for_account(&owner.token, changed.clone());
            let device_push = store.sync_push(&device.token, changed);
            assert!(
                matches!(account, Err(StoreError::Conflict(_))),
                "{account:?}"
            );
            assert!(
                matches!(device_push, Err(StoreError::Conflict(_))),
                "{device_push:?}"
            );
        }
        assert_eq!(fs::read(&path)?, persisted, "a conflict writes nothing");

        // An identical replay is still acknowledged.
        assert!(
            store
                .capture_for_account(&owner.token, original.clone())?
                .duplicate
        );
        let replay = store.sync_push(&device.token, original)?;
        assert!(replay.duplicate && replay.cursor == pushed.cursor);

        // An unowned row (an unscoped capture) is outside sync scope: a conflict, not a 400.
        store.capture_observation(observation("evt-unowned", "unowned content"))?;
        let outside = store.sync_push(
            &device.token,
            observation_for("sess-a", "evt-unowned", "unowned content"),
        );
        assert!(
            matches!(&outside, Err(StoreError::Conflict(message)) if message.contains("outside sync scope")),
            "{outside:?}"
        );
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn adoption_of_an_account_capture_requires_the_same_content() -> Result<(), StoreError> {
        let path = temp_store_path("adopt-parity");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let device =
            store.register_device("device-a", owner.account.person_id.clone(), None, "MacBook")?;
        let original = observation_for("sess-a", "evt-a", "original content");
        let captured = store.capture_for_account(&owner.token, original.clone())?;
        let persisted = fs::read(&path)?;

        // Any changed field is a conflict on the first push too, and nothing is written.
        for changed in [
            ObservationInput {
                content: "changed content".to_owned(),
                ..original.clone()
            },
            ObservationInput {
                session_id: "sess-b".to_owned(),
                ..original.clone()
            },
            ObservationInput {
                role: "user".to_owned(),
                ..original.clone()
            },
            ObservationInput {
                source: "other".to_owned(),
                ..original.clone()
            },
        ] {
            let result = store.sync_push(&device.token, changed);
            assert!(matches!(result, Err(StoreError::Conflict(_))), "{result:?}");
        }
        assert_eq!(fs::read(&path)?, persisted, "a conflict writes nothing");

        // A matching push adopts the stored row once; a replay is then a duplicate.
        let adopted = store.sync_push(&device.token, original.clone())?;
        assert!(adopted.accepted && !adopted.duplicate);
        assert_eq!(adopted.observation, captured.observation);
        let replay = store.sync_push(&device.token, original)?;
        assert!(replay.duplicate && replay.cursor == adopted.cursor);
        assert_eq!(store.observations().count(), 1);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn duplicate_event_is_acknowledged_without_duplication() -> Result<(), StoreError> {
        let path = temp_store_path("deduplication");
        let mut store = MemoryStore::open(&path)?;
        store.start_session("sess-auth-001", context())?;
        let first = store.capture_observation(observation("evt-duplicate", "same event"))?;
        let second = store.capture_observation(observation("evt-duplicate", "same event"))?;

        assert!(!first.duplicate);
        assert!(second.duplicate);
        assert_eq!(store.observations().count(), 1);
        assert_eq!(second.observation.content, "same event");
        let persisted = fs::read(&path)?;
        let repeated = store.capture_observation(observation("evt-duplicate", "same event"))?;
        assert!(repeated.duplicate);
        assert_eq!(fs::read(&path)?, persisted);

        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn failed_end_session_leaves_the_session_open() -> Result<(), StoreError> {
        let path = temp_store_path("end-session-rollback");
        let mut store = MemoryStore::open(&path)?;
        store.start_session("sess-empty", context())?;

        store
            .end_session("sess-empty")
            .expect_err("a session with no observations cannot end");
        assert!(store.sessions().all(|session| session.ended_at.is_none()));

        fs::remove_file(path)?;
        Ok(())
    }

    struct HeldStoreDir {
        parent: PathBuf,
        held: PathBuf,
        released: bool,
    }

    impl HeldStoreDir {
        fn block(store_path: &Path) -> Self {
            let parent = store_path
                .parent()
                .expect("store path has a parent")
                .to_path_buf();
            let held = parent.with_file_name(format!(
                "{}.held",
                parent
                    .file_name()
                    .expect("store dir name")
                    .to_string_lossy()
            ));
            fs::rename(&parent, &held).expect("move store dir aside");
            fs::write(&parent, b"not a directory").expect("replace store dir with a file");
            Self {
                parent,
                held,
                released: false,
            }
        }

        fn release(&mut self) {
            if self.released {
                return;
            }
            if self.parent.is_file() {
                fs::remove_file(&self.parent).expect("remove blocking file");
            }
            if self.held.is_dir() && !self.parent.exists() {
                fs::rename(&self.held, &self.parent).expect("restore store dir");
            }
            self.released = true;
        }
    }

    impl Drop for HeldStoreDir {
        fn drop(&mut self) {
            self.release();
        }
    }

    fn temp_store_dir(test_name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cairn-{test_name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp store dir");
        dir
    }

    #[test]
    fn failed_capture_persist_retries_instead_of_acknowledging_a_duplicate()
    -> Result<(), StoreError> {
        let dir = temp_store_dir("capture-commit");
        let path = dir.join("store.json");
        let mut store = MemoryStore::open(&path)?;
        store.capture_observation(observation("kept-event", "keptalphafact"))?;
        let before = fs::read(&path)?;

        let mut blocked = HeldStoreDir::block(&path);
        let failed = store
            .capture_observation(observation("retry-event", "retrybetafact"))
            .expect_err("a blocked store rename must fail the capture");
        assert!(
            matches!(failed, StoreError::Io(_)),
            "storage error, got {failed}"
        );
        assert!(
            store
                .observations()
                .all(|observation| observation.event_id != "retry-event")
        );
        assert_eq!(
            store
                .observations()
                .filter(|observation| observation.event_id == "kept-event")
                .count(),
            1
        );
        blocked.release();
        assert_eq!(fs::read(&path)?, before);

        let retry = store.capture_observation(observation("retry-event", "retrybetafact"))?;
        assert!(!retry.duplicate);
        assert_eq!(retry.observation.content, "retrybetafact");
        let persisted = fs::read(&path)?;
        let duplicate = store.capture_observation(observation("retry-event", "retrybetafact"))?;
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.observation.content, "retrybetafact");
        assert_eq!(fs::read(&path)?, persisted);

        drop(store);
        let reopened = MemoryStore::open(&path)?;
        assert_eq!(
            reopened
                .observations()
                .filter(|observation| observation.event_id == "retry-event")
                .count(),
            1
        );
        assert_eq!(
            reopened
                .observations()
                .filter(|observation| observation.event_id == "kept-event")
                .count(),
            1
        );
        assert_eq!(reopened.observations().count(), 2);
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    // Runs `op` against a blocked store file, then again once it is released. The failed call
    // must leave the in-memory state untouched and the retry must reach the disk.
    fn failed_write_rolls_back_then_retry_persists(
        store: &mut MemoryStore,
        path: &Path,
        op: impl Fn(&mut MemoryStore) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        let before_state = serde_json::to_value(&store.state)?;
        let before_file = fs::read(path)?;
        let mut blocked = HeldStoreDir::block(path);
        let failed = op(store).expect_err("a blocked rename must fail the write");
        assert!(matches!(failed, StoreError::Io(_)), "got {failed}");
        assert_eq!(serde_json::to_value(&store.state)?, before_state);
        blocked.release();
        assert_eq!(fs::read(path)?, before_file);

        op(store)?;
        assert_ne!(fs::read(path)?, before_file, "the retry must write");
        let reopened = MemoryStore::open(path)?;
        assert_eq!(
            serde_json::to_value(&reopened.state)?,
            serde_json::to_value(&store.state)?
        );
        Ok(())
    }

    #[test]
    fn failed_project_and_team_writes_roll_back_and_retry_persists() -> Result<(), StoreError> {
        let dir = temp_store_dir("project-rollback");
        let path = dir.join("store.json");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let member_id = store
            .register_account("member@example.test", "correct horse battery staple")?
            .account
            .person_id;
        let token = owner.token;
        let member = "member@example.test";

        failed_write_rolls_back_then_retry_persists(&mut store, &path, |s| {
            s.register_project(&token, "cairn-demo", "Demo", None)
                .map(|_| ())
        })?;
        let project_id = store.state.projects[0].project_id.clone();
        let source = remember_with_context(&mut store, &token, "rollback source")?;
        let promoted = store.promote_memory(&token, &project_id, &source.memory_id, "why")?;

        let joining = WorkContext {
            project_id: Some(project_id.clone()),
            ..WorkContext::default()
        };
        failed_write_rolls_back_then_retry_persists(&mut store, &path, |s| {
            s.join_context_project(&member_id, &joining).map(|_| ())
        })?;
        failed_write_rolls_back_then_retry_persists(&mut store, &path, |s| {
            s.share_project_memory(&token, &project_id, &promoted.memory_id, member)
                .map(|_| ())
        })?;
        failed_write_rolls_back_then_retry_persists(&mut store, &path, |s| {
            s.revoke_project_share(&token, &project_id, &promoted.memory_id, member)
                .map(|_| ())
        })?;
        failed_write_rolls_back_then_retry_persists(&mut store, &path, |s| {
            s.create_project_team(&token, &project_id, "Core")
                .map(|_| ())
        })?;
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    // A bundle with a session, a live row, an already-expired row, a tombstoned row and a memory.
    fn import_fixture() -> Result<ExportBundle, StoreError> {
        let path = temp_store_path("import-fixture");
        let mut source = MemoryStore::open(&path)?;
        source.start_session("imp-session", context())?;
        for id in ["imp-live", "imp-expired", "imp-gone"] {
            source.capture_observation(observation_for("imp-session", id, "importfact"))?;
        }
        source.remember("importmemory fact", Vec::new(), context())?;
        let mut bundle = source.export();
        bundle
            .observations
            .iter_mut()
            .find(|observation| observation.event_id == "imp-expired")
            .expect("expired row")
            .lifecycle
            .expires_at = Some("1".to_owned());
        bundle.forgotten_ids = vec!["imp-gone".to_owned()];
        fs::remove_file(path)?;
        Ok(bundle)
    }

    type ImportSnapshot = (
        Vec<String>,
        Vec<String>,
        usize,
        Vec<String>,
        usize,
        usize,
        usize,
    );

    fn import_snapshot(store: &MemoryStore) -> ImportSnapshot {
        let mut forgotten = store
            .state
            .forgotten_ids
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        forgotten.sort();
        (
            store.observations().map(|o| o.event_id.clone()).collect(),
            store
                .state
                .memories
                .iter()
                .map(|m| m.memory_id.clone())
                .collect(),
            store.state.sessions.len(),
            forgotten,
            store.state.observation_owners.len(),
            store.state.memory_owners.len(),
            store.state.sync_deletions.len(),
        )
    }

    #[test]
    fn failed_unscoped_import_persist_changes_nothing_and_retries() -> Result<(), StoreError> {
        let dir = temp_store_dir("import-commit");
        let path = dir.join("store.json");
        let mut store = MemoryStore::open(&path)?;
        store.start_session("base-session", context())?;
        let bundle = import_fixture()?;
        let before = fs::read(&path)?;
        let snapshot = import_snapshot(&store);

        let mut blocked = HeldStoreDir::block(&path);
        let failed = store
            .import_bundle(bundle.clone())
            .expect_err("a blocked store must fail the import");
        assert!(matches!(failed, StoreError::Io(_)), "got {failed}");
        assert_eq!(import_snapshot(&store), snapshot);
        blocked.release();
        assert_eq!(fs::read(&path)?, before);

        // An unrelated write must not commit the failed rows.
        store.start_session("later-session", context())?;
        assert_eq!(MemoryStore::open(&path)?.observations().count(), 0);
        assert_eq!(store.observations().count(), 0);

        let result = store.import_bundle(bundle.clone())?;
        assert_eq!(result.imported_sessions, 1);
        assert_eq!(result.imported_observations, bundle.observations.len());
        assert_eq!(result.imported_memories, bundle.memories.len());
        assert_eq!(result.imported_forgotten_ids, 1);
        let reopened = MemoryStore::open(&path)?;
        assert_eq!(import_snapshot(&reopened), import_snapshot(&store));
        assert!(reopened.state.forgotten_ids.contains("imp-expired"));
        assert!(reopened.state.forgotten_ids.contains("imp-gone"));
        assert!(!reopened.state.forgotten_ids.contains("imp-live"));
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn failed_account_import_persist_changes_nothing_and_retries() -> Result<(), StoreError> {
        let dir = temp_store_dir("account-import-commit");
        let path = dir.join("store.json");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let bundle = import_fixture()?;
        let before = fs::read(&path)?;
        let snapshot = import_snapshot(&store);

        let mut blocked = HeldStoreDir::block(&path);
        let failed = store
            .import_for_account(&owner.token, bundle.clone())
            .expect_err("a blocked store must fail the account import");
        assert!(matches!(failed, StoreError::Io(_)), "got {failed}");
        assert_eq!(import_snapshot(&store), snapshot);
        blocked.release();
        assert_eq!(fs::read(&path)?, before);

        // An unrelated write must not commit rows without their owners.
        store.start_session_for_account(&owner.token, "later-session", context())?;
        assert_eq!(MemoryStore::open(&path)?.observations().count(), 0);
        assert_eq!(store.observations().count(), 0);
        assert!(
            store
                .export_for_account(&owner.token)?
                .observations
                .is_empty()
        );

        let result = store.import_for_account(&owner.token, bundle.clone())?;
        assert_eq!(result.imported_observations, bundle.observations.len());
        assert_eq!(result.imported_memories, bundle.memories.len());
        assert_eq!(result.imported_forgotten_ids, 1);
        let reopened = MemoryStore::open(&path)?;
        assert_eq!(import_snapshot(&reopened), import_snapshot(&store));
        assert_eq!(
            reopened.state.observation_owners.len(),
            bundle.observations.len()
        );
        assert_eq!(reopened.state.memory_owners.len(), bundle.memories.len());
        assert!(reopened.state.forgotten_ids.contains("imp-expired"));
        assert!(reopened.state.forgotten_ids.contains("imp-gone"));
        assert!(reopened.state.sync_deletions.is_empty());
        let exported = reopened.export_for_account(&owner.token)?;
        assert_eq!(exported.observations.len(), bundle.observations.len());
        assert!(exported.forgotten_ids.iter().any(|id| id == "imp-expired"));
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn failed_account_capture_persist_leaves_no_unowned_observation() -> Result<(), StoreError> {
        let dir = temp_store_dir("account-capture-commit");
        let path = dir.join("store.json");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let member =
            store.register_account("member@example.test", "correct horse battery staple")?;
        store.start_session_for_account(&owner.token, "sess-commit", WorkContext::default())?;
        store.capture_for_account(
            &owner.token,
            observation_for("sess-commit", "kept-event", "keptalphafact"),
        )?;
        let before = fs::read(&path)?;
        let owners_before = store.state.observation_owners.len();

        let mut blocked = HeldStoreDir::block(&path);
        let failed = store
            .capture_for_account(
                &owner.token,
                observation_for("sess-commit", "retry-event", "retrybetafact"),
            )
            .expect_err("a blocked store rename must fail the account capture");
        assert!(
            matches!(failed, StoreError::Io(_)),
            "storage error, got {failed}"
        );
        assert!(
            store
                .observations()
                .all(|observation| observation.event_id != "retry-event")
        );
        assert!(!store.observation_owned_by("retry-event", &owner.account.person_id));
        assert_eq!(store.state.observation_owners.len(), owners_before);
        assert_eq!(
            store
                .observations()
                .filter(|observation| observation.event_id == "kept-event")
                .count(),
            1
        );
        blocked.release();
        assert_eq!(fs::read(&path)?, before);

        let retry = store.capture_for_account(
            &owner.token,
            observation_for("sess-commit", "retry-event", "retrybetafact"),
        )?;
        assert!(!retry.duplicate);
        assert_eq!(retry.observation.content, "retrybetafact");
        assert!(store.observation_owned_by("retry-event", &owner.account.person_id));
        assert!(!store.observation_owned_by("retry-event", &member.account.person_id));
        let persisted = fs::read(&path)?;
        let duplicate = store.capture_for_account(
            &owner.token,
            observation_for("sess-commit", "retry-event", "retrybetafact"),
        )?;
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.observation.content, "retrybetafact");
        assert_eq!(fs::read(&path)?, persisted);

        let foreign = store
            .capture_for_account(
                &member.token,
                observation_for("sess-commit", "retry-event", "member marker"),
            )
            .expect_err("foreign event stays rejected");
        let foreign_text = foreign.to_string();
        assert!(foreign_text.contains("event belongs to another person: retry-event"));
        assert!(!foreign_text.contains("retrybetafact"));
        assert_eq!(fs::read(&path)?, persisted);

        store.end_session_for_account(&owner.token, "sess-commit")?;
        drop(store);
        let reopened = MemoryStore::open(&path)?;
        assert_eq!(
            reopened
                .observations()
                .filter(|observation| observation.event_id == "retry-event")
                .count(),
            1
        );
        assert!(reopened.observation_owned_by("retry-event", &owner.account.person_id));
        assert!(!reopened.observation_owned_by("retry-event", &member.account.person_id));
        let hits = reopened.search_for_account(
            &owner.token,
            "retrybetafact",
            5,
            &RecallFilter::default(),
        )?;
        assert_eq!(hits.len(), 1);
        assert!(hits[0].content.contains("retrybetafact"));
        let member_hits = reopened.search_for_account(
            &member.token,
            "retrybetafact",
            5,
            &RecallFilter::default(),
        )?;
        assert!(member_hits.is_empty());
        assert_eq!(
            reopened
                .observations()
                .filter(|observation| observation.event_id == "kept-event")
                .count(),
            1
        );
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn failed_account_correction_persist_leaves_no_unowned_successor() -> Result<(), StoreError> {
        let dir = temp_store_dir("account-correct-commit");
        let path = dir.join("store.json");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        remember_owned(
            &mut store,
            &owner.token,
            "correctionalphafact stays recalled",
        )?;
        let target = store
            .search_for_account(
                &owner.token,
                "correctionalphafact",
                5,
                &RecallFilter::default(),
            )?
            .remove(0)
            .memory_id;
        let before = fs::read(&path)?;
        let memories_before = store.state.memories.len();
        let owners_before = store.state.memory_owners.len();
        let sources_before = store.state.observation_owners.len();

        let mut blocked = HeldStoreDir::block(&path);
        let failed = store
            .correct_for_account(
                &owner.token,
                &target,
                "correctionbetafact replaces it",
                Vec::new(),
                WorkContext::default(),
                LifecycleInput::default(),
            )
            .expect_err("a blocked store rename must fail the account correction");
        assert!(
            matches!(failed, StoreError::Io(_)),
            "storage error, got {failed}"
        );
        // An unowned successor would hide the owner's target from recall.
        assert_eq!(store.state.memories.len(), memories_before);
        assert_eq!(store.state.memory_owners.len(), owners_before);
        assert_eq!(store.state.observation_owners.len(), sources_before);
        let recalled = store.search_for_account(
            &owner.token,
            "correctionalphafact",
            5,
            &RecallFilter::default(),
        )?;
        assert_eq!(recalled.len(), 1);
        blocked.release();
        assert_eq!(fs::read(&path)?, before);

        let retry = store.correct_for_account(
            &owner.token,
            &target,
            "correctionbetafact replaces it",
            Vec::new(),
            WorkContext::default(),
            LifecycleInput::default(),
        )?;
        assert!(store.memory_owned_by(&retry.memory_id, &owner.account.person_id));
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn failed_sync_commit_does_not_keep_an_observation_without_sync_metadata()
    -> Result<(), StoreError> {
        let dir = temp_store_dir("sync-capture-commit");
        let path = dir.join("store.json");
        let mut store = MemoryStore::open(&path)?;
        let owner_account =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let other_account =
            store.register_account("other@example.test", "correct horse battery staple")?;
        let owner = store.register_device(
            "device-owner",
            &owner_account.account.person_id,
            None,
            "Owner",
        )?;
        let other = store.register_device(
            "device-other",
            &other_account.account.person_id,
            None,
            "Other",
        )?;
        store.sync_push(
            &owner.token,
            observation_for("sync-sess", "kept-sync", "keptalphafact"),
        )?;
        let seen_before = "baseline-seen";
        assert_ne!(seen_before, timestamp());
        store
            .state
            .devices
            .iter_mut()
            .find(|record| record.device.device_id == "device-owner")
            .expect("owner device")
            .device
            .last_seen_at = seen_before.to_owned();
        store.persist()?;
        let before = fs::read(&path)?;
        let cursor_before = store.state.next_sync_cursor;
        let events_before = store.state.sync_events.len();

        let mut blocked = HeldStoreDir::block(&path);
        let failed = store
            .sync_push(
                &owner.token,
                observation_for("sync-sess", "retry-sync", "retrybetafact"),
            )
            .expect_err("a blocked parent directory must fail the sync commit");
        assert!(
            matches!(failed, StoreError::Io(_)),
            "storage error, got {failed}"
        );
        assert_eq!(
            store
                .state
                .devices
                .iter()
                .find(|record| record.device.device_id == "device-owner")
                .expect("owner device")
                .device
                .last_seen_at,
            seen_before
        );
        assert!(
            store
                .observations()
                .all(|observation| observation.event_id != "retry-sync")
        );
        assert!(
            store
                .state
                .sync_events
                .iter()
                .all(|event| event.event_id != "retry-sync")
        );
        assert_eq!(store.state.next_sync_cursor, cursor_before);
        assert_eq!(store.state.sync_events.len(), events_before);
        assert_eq!(
            store
                .observations()
                .filter(|observation| observation.event_id == "kept-sync")
                .count(),
            1
        );
        blocked.release();
        assert_eq!(fs::read(&path)?, before);

        let retry = store.sync_push(
            &owner.token,
            observation_for("sync-sess", "retry-sync", "retrybetafact"),
        )?;
        assert!(!retry.duplicate);
        assert_eq!(retry.observation.content, "retrybetafact");
        assert_eq!(
            store
                .state
                .sync_events
                .iter()
                .filter(|event| event.event_id == "retry-sync"
                    && event.person_id == owner_account.account.person_id
                    && event.cursor == retry.cursor)
                .count(),
            1
        );

        drop(store);
        let mut reopened = MemoryStore::open(&path)?;
        assert_eq!(
            reopened
                .observations()
                .filter(|observation| observation.event_id == "retry-sync")
                .count(),
            1
        );
        assert_eq!(
            reopened
                .state
                .sync_events
                .iter()
                .filter(|event| event.event_id == "retry-sync"
                    && event.person_id == owner_account.account.person_id)
                .count(),
            1
        );
        assert_eq!(
            reopened
                .observations()
                .filter(|observation| observation.event_id == "kept-sync")
                .count(),
            1
        );
        let pulled = reopened.sync_pull(&owner.token, 0)?;
        assert_eq!(
            pulled
                .events
                .iter()
                .filter(|event| event.event_id == "retry-sync")
                .count(),
            1
        );
        assert_eq!(
            pulled
                .events
                .iter()
                .filter(|event| event.event_id == "kept-sync")
                .count(),
            1
        );
        let collision = reopened
            .sync_push(
                &other.token,
                observation_for("other-sess", "retry-sync", "othermarkertext"),
            )
            .expect_err("another person still cannot claim the event");
        let collision_text = collision.to_string();
        assert!(collision_text.contains("event belongs to another person: retry-sync"));
        assert!(!collision_text.contains("retrybetafact"));
        assert!(!collision_text.contains("othermarkertext"));
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    fn owner_pairs(store: &MemoryStore) -> Vec<(String, String)> {
        store
            .state
            .observation_owners
            .iter()
            .map(|record| (record.event_id.clone(), record.person_id.clone()))
            .collect()
    }

    #[test]
    fn owned_observation_adopts_one_sync_event() -> Result<(), StoreError> {
        let dir = temp_store_dir("sync-adopt");
        let path = dir.join("store.json");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let member =
            store.register_account("member@example.test", "correct horse battery staple")?;
        let owner_context = WorkContext {
            repository: Some("owner-repo".to_owned()),
            branch: Some("owner-branch".to_owned()),
            ..WorkContext::default()
        };
        let member_context = WorkContext {
            repository: Some("member-repo".to_owned()),
            branch: Some("member-branch".to_owned()),
            ..WorkContext::default()
        };
        store.start_session(
            "adopt-session",
            WorkContext {
                repository: Some("legacy-repo".to_owned()),
                branch: Some("legacy-branch".to_owned()),
                ..WorkContext::default()
            },
        )?;
        store.start_session_for_account(&owner.token, "adopt-session", owner_context)?;
        store.start_session_for_account(&member.token, "adopt-session", member_context.clone())?;
        let captured = store.capture_for_account(
            &owner.token,
            empty_capture("adopt-session", "owned-event", "canonicalalphafact"),
        )?;
        assert!(!captured.duplicate);
        assert_eq!(
            captured.observation.context.branch.as_deref(),
            Some("owner-branch")
        );
        let canonical = captured.observation.clone();
        let owner_device =
            store.register_device("device-owner", &owner.account.person_id, None, "Owner")?;
        let owner_other = store.register_device(
            "device-owner-2",
            &owner.account.person_id,
            None,
            "Owner two",
        )?;
        let member_device =
            store.register_device("device-member", &member.account.person_id, None, "Member")?;

        store.capture_observation(observation_for(
            "anon-sess",
            "anon-event",
            "anonymousmarkerfact",
        ))?;
        let owners_before_reject = owner_pairs(&store);
        let anon = store
            .sync_push(
                &owner_device.token,
                ObservationInput {
                    content: owner.account.person_id.clone(),
                    context: member_context.clone(),
                    ..observation_for("anon-sess", "anon-event", "claimtext")
                },
            )
            .expect_err("an unowned observation is not adopted");
        let anon_text = anon.to_string();
        assert!(
            anon_text.contains("event already exists outside sync scope: anon-event"),
            "{anon_text}"
        );
        assert!(!anon_text.contains("anonymousmarkerfact"));
        assert!(!anon_text.contains("claimtext"));
        assert!(!anon_text.contains(&owner.account.person_id));
        assert_eq!(owner_pairs(&store), owners_before_reject);
        assert!(
            store
                .state
                .sync_events
                .iter()
                .all(|event| event.event_id != "anon-event")
        );

        store.capture_for_account(
            &member.token,
            observation_for("adopt-session", "member-event", "memberprivatefact"),
        )?;
        let foreign = store
            .sync_push(
                &owner_device.token,
                ObservationInput {
                    content: owner.account.person_id.clone(),
                    context: owner_context_for_claim(),
                    session_id: "adopt-session".to_owned(),
                    ..observation_for("adopt-session", "member-event", "stealtext")
                },
            )
            .expect_err("another person's observation is not acknowledged");
        let foreign_text = foreign.to_string();
        assert!(
            foreign_text.contains("event belongs to another person: member-event"),
            "{foreign_text}"
        );
        assert!(!foreign_text.contains("memberprivatefact"));
        assert!(!foreign_text.contains("stealtext"));
        assert!(!foreign_text.contains(&member.account.person_id));
        assert!(store.observation_owned_by("member-event", &member.account.person_id));
        assert!(!store.observation_owned_by("member-event", &owner.account.person_id));
        assert!(
            store
                .state
                .sync_events
                .iter()
                .all(|event| event.event_id != "member-event")
        );

        let replacement = ObservationInput {
            event_id: "owned-event".to_owned(),
            session_id: "other-session".to_owned(),
            content: "replacementbetatext".to_owned(),
            role: "user".to_owned(),
            source: "sync".to_owned(),
            importance: Some("low".to_owned()),
            lifecycle: LifecycleInput::default(),
            context: member_context,
        };
        // Changed text is not the same event: a conflict, and nothing is attached.
        let rejected = store.sync_push(&owner_device.token, replacement.clone());
        assert!(
            matches!(rejected, Err(StoreError::Conflict(_))),
            "{rejected:?}"
        );
        assert!(
            store
                .state
                .sync_events
                .iter()
                .all(|event| event.event_id != "owned-event")
        );
        // First attachment of the same event enters the sync stream, so this is not a duplicate.
        let adopted = store.sync_push(
            &owner_device.token,
            empty_capture("adopt-session", "owned-event", "canonicalalphafact"),
        )?;
        assert!(adopted.accepted);
        assert!(!adopted.duplicate);
        assert_eq!(adopted.observation, canonical);
        assert_eq!(
            store
                .observations()
                .filter(|observation| observation.event_id == "owned-event")
                .count(),
            1
        );
        assert_eq!(
            store
                .state
                .sync_events
                .iter()
                .filter(|event| event.event_id == "owned-event"
                    && event.person_id == owner.account.person_id
                    && event.cursor == adopted.cursor
                    && event.observation == canonical)
                .count(),
            1
        );
        assert!(store.observation_owned_by("owned-event", &owner.account.person_id));
        let persisted = fs::read_to_string(&path)?;
        assert!(persisted.contains("canonicalalphafact"));
        assert!(!persisted.contains("replacementbetatext"));
        assert!(!persisted.contains("other-session"));

        // Replaying the pushed text after adoption is a changed replay of the canonical row.
        let changed = store.sync_push(&owner_device.token, replacement);
        assert!(
            matches!(changed, Err(StoreError::Conflict(_))),
            "{changed:?}"
        );
        let retry = store.sync_push(
            &owner_device.token,
            empty_capture("adopt-session", "owned-event", "canonicalalphafact"),
        )?;
        assert!(retry.duplicate);
        assert_eq!(retry.cursor, adopted.cursor);
        assert_eq!(retry.observation, canonical);
        assert_eq!(
            store
                .state
                .sync_events
                .iter()
                .filter(|event| event.event_id == "owned-event")
                .count(),
            1
        );
        assert!(!fs::read_to_string(&path)?.contains("replacementbetatext"));

        let pulled = store.sync_pull(&owner_other.token, 0)?;
        assert_eq!(
            pulled
                .events
                .iter()
                .filter(|event| event.event_id == "owned-event" && event.observation == canonical)
                .count(),
            1
        );
        let resumed = store.sync_pull(&owner_other.token, pulled.cursor)?;
        assert!(
            resumed
                .events
                .iter()
                .all(|event| event.event_id != "owned-event")
        );

        let collision = store
            .sync_push(
                &member_device.token,
                observation_for("adopt-session", "owned-event", "memberstealtext"),
            )
            .expect_err("another person's device cannot acknowledge the event");
        let collision_text = collision.to_string();
        assert!(collision_text.contains("event belongs to another person: owned-event"));
        assert!(!collision_text.contains("canonicalalphafact"));
        assert!(!collision_text.contains("memberstealtext"));

        let fresh = store.sync_push(
            &owner_device.token,
            empty_capture("adopt-session", "fresh-sync", "freshsyncfact"),
        )?;
        assert!(!fresh.duplicate);
        assert_eq!(
            fresh.observation.context.branch.as_deref(),
            Some("owner-branch")
        );
        assert_ne!(
            fresh.observation.context.repository.as_deref(),
            Some("legacy-repo")
        );
        assert_ne!(
            fresh.observation.context.branch.as_deref(),
            Some("member-branch")
        );

        drop(store);
        let mut reopened = MemoryStore::open(&path)?;
        assert_eq!(
            reopened
                .observations()
                .find(|observation| observation.event_id == "owned-event")
                .expect("canonical row")
                .content,
            "canonicalalphafact"
        );
        assert_eq!(
            reopened
                .observations()
                .filter(|observation| observation.event_id == "owned-event")
                .count(),
            1
        );
        assert!(reopened.observation_owned_by("owned-event", &owner.account.person_id));
        assert_eq!(
            reopened
                .state
                .sync_events
                .iter()
                .filter(|event| event.event_id == "owned-event"
                    && event.person_id == owner.account.person_id)
                .count(),
            1
        );
        let pulled_again = reopened.sync_pull(&owner_device.token, 0)?;
        assert_eq!(
            pulled_again
                .events
                .iter()
                .filter(|event| event.event_id == "owned-event")
                .count(),
            1
        );
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    fn owner_context_for_claim() -> WorkContext {
        WorkContext {
            repository: Some("owner-repo".to_owned()),
            branch: Some("owner-branch".to_owned()),
            ..WorkContext::default()
        }
    }

    #[test]
    fn failed_sync_adoption_keeps_the_observation_and_retries() -> Result<(), StoreError> {
        let dir = temp_store_dir("sync-adopt-commit");
        let path = dir.join("store.json");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let member =
            store.register_account("member@example.test", "correct horse battery staple")?;
        let captured = store.capture_for_account(
            &owner.token,
            observation_for("adopt-sess", "owned-event", "canonicalalphafact"),
        )?;
        let canonical = captured.observation.clone();
        let device =
            store.register_device("device-owner", &owner.account.person_id, None, "Owner")?;
        let other =
            store.register_device("device-member", &member.account.person_id, None, "Member")?;
        let seen_before = "baseline-seen";
        store
            .state
            .devices
            .iter_mut()
            .find(|record| record.device.device_id == "device-owner")
            .expect("owner device")
            .device
            .last_seen_at = seen_before.to_owned();
        store.persist()?;
        let before = fs::read(&path)?;
        let owners_before = owner_pairs(&store);
        let cursor_before = store.state.next_sync_cursor;
        let events_before = store.state.sync_events.len();

        let mut blocked = HeldStoreDir::block(&path);
        let failed = store
            .sync_push(
                &device.token,
                observation_for("adopt-sess", "owned-event", "canonicalalphafact"),
            )
            .expect_err("a blocked parent directory must fail the adoption commit");
        assert!(
            matches!(failed, StoreError::Io(_)),
            "storage error, got {failed}"
        );
        assert_eq!(
            store
                .state
                .devices
                .iter()
                .find(|record| record.device.device_id == "device-owner")
                .expect("owner device")
                .device
                .last_seen_at,
            seen_before
        );
        assert_eq!(store.state.next_sync_cursor, cursor_before);
        assert_eq!(store.state.sync_events.len(), events_before);
        assert!(
            store
                .state
                .sync_events
                .iter()
                .all(|event| event.event_id != "owned-event")
        );
        assert_eq!(
            store
                .observations()
                .find(|observation| observation.event_id == "owned-event")
                .cloned(),
            Some(canonical.clone())
        );
        assert_eq!(owner_pairs(&store), owners_before);
        assert!(store.observation_owned_by("owned-event", &owner.account.person_id));
        blocked.release();
        assert_eq!(fs::read(&path)?, before);

        let retry = store.sync_push(
            &device.token,
            observation_for("adopt-sess", "owned-event", "canonicalalphafact"),
        )?;
        assert!(!retry.duplicate);
        assert_eq!(retry.observation, canonical);
        assert_eq!(
            store
                .state
                .sync_events
                .iter()
                .filter(|event| event.event_id == "owned-event"
                    && event.cursor == retry.cursor
                    && event.person_id == owner.account.person_id)
                .count(),
            1
        );
        let changed = store.sync_push(
            &device.token,
            observation_for("other-sess", "owned-event", "latermarkertext"),
        );
        assert!(
            matches!(changed, Err(StoreError::Conflict(_))),
            "{changed:?}"
        );
        let again = store.sync_push(
            &device.token,
            empty_capture("adopt-sess", "owned-event", "canonicalalphafact"),
        )?;
        assert!(again.duplicate);
        assert_eq!(again.cursor, retry.cursor);
        assert_eq!(again.observation, canonical);

        drop(store);
        let mut reopened = MemoryStore::open(&path)?;
        assert_eq!(
            reopened
                .observations()
                .find(|observation| observation.event_id == "owned-event")
                .expect("canonical row")
                .content,
            "canonicalalphafact"
        );
        assert!(reopened.observation_owned_by("owned-event", &owner.account.person_id));
        assert_eq!(
            reopened
                .state
                .sync_events
                .iter()
                .filter(|event| event.event_id == "owned-event")
                .count(),
            1
        );
        let pulled = reopened.sync_pull(&device.token, 0)?;
        assert_eq!(
            pulled
                .events
                .iter()
                .filter(|event| event.event_id == "owned-event"
                    && event.observation.content == "canonicalalphafact")
                .count(),
            1
        );
        let collision = reopened
            .sync_push(
                &other.token,
                observation_for("adopt-sess", "owned-event", "memberstealtext"),
            )
            .expect_err("a foreign device still cannot acknowledge the event");
        let collision_text = collision.to_string();
        assert!(collision_text.contains("event belongs to another person: owned-event"));
        assert!(!collision_text.contains("canonicalalphafact"));
        let _ = fs::remove_dir_all(&dir);
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

    #[test]
    fn a_project_with_a_member_needs_the_web_session_to_merge() -> Result<(), StoreError> {
        let path = temp_store_path("device-merge-gate");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let member =
            store.register_account("member@example.test", "correct horse battery staple")?;
        let device =
            store.register_device("device-own", &owner.account.person_id, None, "Laptop")?;
        let (project_id, _) = owned_project(&mut store, &device.token, "device remove decision")?;
        let branch =
            store.register_branch(&device.token, &project_id, "feat/auth", None, None, None)?;
        let source = remember_with_context(&mut store, &device.token, "gate merge source")?;
        join_project(&mut store, &member.token, &project_id)?;

        let blocked = store
            .confirm_branch_merge(
                &device.token,
                &project_id,
                &branch.branch_id,
                &source.memory_id,
                "c1",
                "gate",
            )
            .expect_err("a project with members still needs the web console");
        assert!(blocked.to_string().contains(WEB_ONLY_ERROR));
        store.confirm_branch_merge(
            &owner.token,
            &project_id,
            &branch.branch_id,
            &source.memory_id,
            "c1",
            "gate",
        )?;
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn device_tokens_are_stored_hashed_and_reregistering_rotates() -> Result<(), StoreError> {
        let path = temp_store_path("device-token-hash");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let person_id = owner.account.person_id.clone();
        let first = store.register_device("device-a", person_id.clone(), None, "Laptop")?;
        let second = store.register_device("device-a", person_id, None, "Laptop")?;
        assert!(second.token.starts_with(DEVICE_TOKEN_PREFIX));
        assert_ne!(first.token, second.token);
        assert!(store.authenticate_device(&first.token).is_err());
        assert!(store.authenticate_device(&second.token).is_ok());
        assert!(!store.is_device_token(&first.token));

        let on_disk = fs::read_to_string(&path)?;
        assert!(!on_disk.contains(&first.token));
        assert!(!on_disk.contains(&second.token));
        assert!(on_disk.contains(&hash_token(&second.token)));
        let reopened = MemoryStore::open(&path)?;
        assert!(reopened.is_device_token(&second.token));
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn sync_refuses_a_device_of_a_disabled_account() -> Result<(), StoreError> {
        let path = temp_store_path("device-disabled-account");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let device = store.register_device("device-a", &owner.account.person_id, None, "Laptop")?;
        store.state.accounts[0].account.status = AccountStatus::Disabled;
        assert!(store.sync_pull(&device.token, 0).is_err());
        assert!(store.sync_diagnostics(&device.token).is_err());
        assert!(
            store
                .sync_push(
                    &device.token,
                    observation_for("s", "disabled-event", "nope")
                )
                .is_err()
        );
        assert!(store.authenticate_account(&device.token).is_err());
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn read_only_sync_calls_do_not_rewrite_the_store() -> Result<(), StoreError> {
        let path = temp_store_path("sync-read-only");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let device = store.register_device("device-a", &owner.account.person_id, None, "Laptop")?;
        // Pretend the last use was long ago so only the throttle could skip the write.
        store.state.devices[0].device.last_seen_at = "1".to_owned();
        store.persist()?;
        let before = fs::read(&path)?;

        store.sync_pull(&device.token, 0)?;
        store.sync_diagnostics(&device.token)?;
        assert_eq!(fs::read(&path)?, before);

        // A fresh last-seen value is not rewritten within the throttle window.
        store.touch_device(&device.token);
        let seen = store.state.devices[0].device.last_seen_at.clone();
        assert_ne!(seen, "1");
        store.touch_device(&device.token);
        assert_eq!(store.state.devices[0].device.last_seen_at, seen);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn revoked_device_cannot_be_registered_again() -> Result<(), StoreError> {
        let path = temp_store_path("revoked-device-register");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let person_id = owner.account.person_id.clone();
        let device = store.register_device("device-a", person_id.clone(), None, "Laptop")?;
        store.revoke_device(&owner.token, "device-a")?;
        let error = store
            .register_device("device-a", person_id, None, "Laptop")
            .expect_err("a revoked device id is not reusable");
        assert!(
            error
                .to_string()
                .contains("device is revoked; register a new device id")
        );
        assert!(store.authenticate_device(&device.token).is_err());
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn repromoting_a_forgotten_copy_creates_a_live_copy() -> Result<(), StoreError> {
        let path = temp_store_path("repromote-forgotten");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let (project_id, first) =
            owned_project(&mut store, &owner.token, "repromote personal fact")?;
        let source_id = first.promoted_from.clone().expect("origin");
        store.forget_for_account(&owner.token, &first.memory_id)?;
        let second = store.promote_memory(&owner.token, &project_id, &source_id, "again")?;
        assert_ne!(second.memory_id, first.memory_id);
        assert!(!store.state.forgotten_ids.contains(&second.memory_id));
        let listed = store.project_memories(&owner.token, &project_id)?;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].memory_id, second.memory_id);
        // A live copy is still returned unchanged on a repeat.
        let repeat = store.promote_memory(&owner.token, &project_id, &source_id, "again")?;
        assert_eq!(repeat.memory_id, second.memory_id);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn contradicting_equal_scores_rank_the_newer_memory_first() -> Result<(), StoreError> {
        let path = temp_store_path("contradiction-newer-first");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let filter = RecallFilter::default();
        let ctx = WorkContext {
            repository: Some("cairn-demo".to_owned()),
            branch: Some("main".to_owned()),
            ..WorkContext::default()
        };
        let mut ids = Vec::new();
        for content in [
            "gammaquota widget does apply",
            "gammaquota widget does not apply",
        ] {
            ids.push(
                store
                    .remember_for_account(
                        &owner.token,
                        content,
                        Vec::new(),
                        ctx.clone(),
                        LifecycleInput::default(),
                        MemoryKind::Explicit,
                    )?
                    .memory_id,
            );
        }
        let ranked = |store: &MemoryStore| -> Result<Vec<String>, StoreError> {
            let hits = store.search_for_account(&owner.token, "gammaquota", 5, &filter)?;
            assert!(hits.iter().all(|hit| hit.contradictory));
            let unscoped = store.search_with_filter("gammaquota", 5, &filter)?;
            assert_eq!(
                hits.iter().map(|hit| &hit.memory_id).collect::<Vec<_>>(),
                unscoped
                    .iter()
                    .map(|hit| &hit.memory_id)
                    .collect::<Vec<_>>()
            );
            Ok(hits.into_iter().map(|hit| hit.memory_id).collect())
        };
        // Whichever is newer comes first, so the order follows time and not the id.
        for newer_id in &ids {
            for memory in &mut store.state.memories {
                memory.created_at = if memory.memory_id == *newer_id {
                    "2000"
                } else {
                    "1000"
                }
                .to_owned();
            }
            let order = ranked(&store)?;
            assert_eq!(&order[0], newer_id);
            assert_eq!(order.len(), 2);
        }
        fs::remove_file(path)?;
        Ok(())
    }

    fn age_observations(store: &mut MemoryStore, session_id: &str, secs: u64) {
        let at = (now_seconds() - secs).to_string();
        for observation in &mut store.state.observations {
            if observation.session_id == session_id {
                observation.captured_at.clone_from(&at);
            }
        }
    }

    fn ended(store: &MemoryStore, person_id: &str, session_id: &str) -> bool {
        store
            .state
            .sessions
            .iter()
            .find(|s| s.session_id == session_id && s.person_id.as_deref() == Some(person_id))
            .is_some_and(|s| s.ended_at.is_some())
    }

    fn summaries<'a>(store: &'a MemoryStore, session_id: &str) -> Vec<&'a Memory> {
        store
            .state
            .memories
            .iter()
            .filter(|m| m.kind == MemoryKind::SessionSummary && m.session_id == session_id)
            .collect()
    }

    #[test]
    fn capture_finishes_only_this_persons_idle_sessions() -> Result<(), StoreError> {
        let path = temp_store_path("auto-close-idle");
        let mut store = MemoryStore::open(&path)?;
        let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
        let b = store.register_account("b@example.test", "correct horse battery staple")?;
        let (a_id, b_id) = (a.account.person_id.clone(), b.account.person_id.clone());
        for (token, session) in [
            (&a.token, "idle-a"),
            (&a.token, "fresh-a"),
            (&b.token, "idle-b"),
        ] {
            store.start_session_for_account(token, session, WorkContext::default())?;
            store.capture_for_account(
                token,
                observation_for(session, &format!("evt-{session}"), "idlefact"),
            )?;
        }
        store.start_session_for_account(&a.token, "empty-a", WorkContext::default())?;
        age_observations(&mut store, "idle-a", 31 * 60);
        age_observations(&mut store, "idle-b", 31 * 60);
        age_observations(&mut store, "fresh-a", 29 * 60);
        assert!(!ended(&store, &a_id, "idle-a"));

        store.capture_for_account(
            &a.token,
            observation_for("trigger-a", "evt-trigger", "trigger"),
        )?;

        assert!(ended(&store, &a_id, "idle-a"));
        assert_eq!(summaries(&store, "idle-a").len(), 1);
        assert!(store.memory_owned_by(&summaries(&store, "idle-a")[0].memory_id.clone(), &a_id));
        assert!(!ended(&store, &a_id, "fresh-a"), "29 minutes is not idle");
        assert!(
            !ended(&store, &a_id, "empty-a"),
            "no observations: skipped, no error"
        );
        assert!(summaries(&store, "empty-a").is_empty());
        assert!(
            !ended(&store, &b_id, "idle-b"),
            "another person is never swept"
        );
        assert!(summaries(&store, "idle-b").is_empty());
        // A session start by the other person sweeps only that person.
        store.start_session_for_account(&b.token, "next-b", WorkContext::default())?;
        assert!(ended(&store, &b_id, "idle-b"));
        assert!(!ended(&store, &a_id, "fresh-a"));
        // The sweep is persisted.
        let reopened = MemoryStore::open(&path)?;
        assert!(ended(&reopened, &a_id, "idle-a") && ended(&reopened, &b_id, "idle-b"));
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn reused_session_id_gets_a_fresh_summary() -> Result<(), StoreError> {
        let path = temp_store_path("auto-close-reuse");
        let mut store = MemoryStore::open(&path)?;
        let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
        let a_id = a.account.person_id.clone();
        store.start_session_for_account(&a.token, "reuse", WorkContext::default())?;
        store.capture_for_account(
            &a.token,
            observation_for("reuse", "evt-1", "firstreusefact"),
        )?;
        let first = store.end_session_for_account(&a.token, "reuse")?;
        assert_eq!(first.source_event_ids.len(), 1);
        // The agent keeps using the ended session id.
        store.capture_for_account(
            &a.token,
            observation_for("reuse", "evt-2", "secondreusefact"),
        )?;
        // Idle sweep: the stale summary is derived again and replaced.
        age_observations(&mut store, "reuse", 31 * 60);
        store.capture_for_account(&a.token, observation_for("other", "evt-3", "other"))?;
        let summary = summaries(&store, "reuse");
        assert_eq!(summary.len(), 1, "replaced, not duplicated");
        assert_eq!(summary[0].memory_id, first.memory_id);
        assert_eq!(summary[0].source_event_ids.len(), 2);
        assert!(summary[0].content.contains("secondreusefact"));
        // An unchanged summary is left alone by a repeat.
        let same = store.end_session_for_account(&a.token, "reuse")?;
        assert_eq!(&same, summaries(&store, "reuse")[0]);
        // An explicit end re-derives too.
        store.capture_for_account(
            &a.token,
            observation_for("reuse", "evt-4", "thirdreusefact"),
        )?;
        let third = store.end_session_for_account(&a.token, "reuse")?;
        assert_eq!(third.source_event_ids.len(), 3);
        assert!(third.content.contains("thirdreusefact"));
        assert_eq!(summaries(&store, "reuse").len(), 1);
        assert!(store.memory_owned_by(&third.memory_id, &a_id));
        fs::remove_file(path)?;
        Ok(())
    }

    fn repo_context(repository: &str, branch: Option<&str>) -> WorkContext {
        WorkContext {
            repository: Some(repository.to_owned()),
            branch: branch.map(ToOwned::to_owned),
            ..WorkContext::default()
        }
    }

    fn capture_in(
        store: &mut MemoryStore,
        token: &str,
        event_id: &str,
        context: WorkContext,
    ) -> Result<(), StoreError> {
        let mut input = observation_for("reg-session", event_id, "regfact");
        input.context = context;
        store.capture_for_account(token, input)?;
        Ok(())
    }

    #[test]
    fn capture_registers_project_and_branch_once() -> Result<(), StoreError> {
        let path = temp_store_path("auto-register");
        let mut store = MemoryStore::open(&path)?;
        let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
        capture_in(
            &mut store,
            &a.token,
            "e1",
            repo_context("  auto-repo ", Some("feat/x")),
        )?;
        capture_in(
            &mut store,
            &a.token,
            "e2",
            repo_context("auto-repo", Some("feat/x")),
        )?;
        capture_in(
            &mut store,
            &a.token,
            "e3",
            repo_context("auto-repo", Some("HEAD")),
        )?;
        capture_in(&mut store, &a.token, "e4", repo_context("auto-repo", None))?;
        capture_in(
            &mut store,
            &a.token,
            "e5",
            repo_context("auto-repo", Some("  ")),
        )?;
        capture_in(&mut store, &a.token, "e6", WorkContext::default())?;
        let projects = store.projects(&a.token)?;
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].repository, "auto-repo");
        let project_id = projects[0].project_id.clone();
        assert_eq!(
            store.project_owner_id(&project_id).as_deref(),
            Some(a.account.person_id.as_str())
        );
        let branches = store.branches(&a.token, &project_id)?;
        assert_eq!(branches.len(), 1);
        assert_eq!(branches[0].name, "feat/x");
        assert_eq!(branches[0].status, "active");
        // The explicit registration returns the same project.
        let explicit = store.register_project(&a.token, "auto-repo", "Auto", None)?;
        assert_eq!(explicit.project_id, project_id);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn capture_uses_a_shared_project_and_never_claims_an_ownerless_one() -> Result<(), StoreError> {
        let path = temp_store_path("auto-register-member");
        let mut store = MemoryStore::open(&path)?;
        let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
        let member = store.register_account("m@example.test", "correct horse battery staple")?;
        let outsider = store.register_account("x@example.test", "correct horse battery staple")?;
        let shared = store.register_project(&owner.token, "shared-repo", "Shared", None)?;
        join_project(&mut store, &member.token, &shared.project_id)?;

        capture_in(
            &mut store,
            &member.token,
            "m1",
            repo_context("shared-repo", Some("feat/m")),
        )?;
        assert_eq!(
            store.projects(&member.token)?.len(),
            1,
            "no second project for the member"
        );
        assert_eq!(store.state.projects.len(), 1);
        assert!(
            store.state.project_branches.is_empty(),
            "branch writes stay owner-only"
        );

        capture_in(
            &mut store,
            &owner.token,
            "o1",
            repo_context("shared-repo", Some("feat/o")),
        )?;
        assert_eq!(store.branches(&owner.token, &shared.project_id)?.len(), 1);

        // A project with no owner record stays ownerless; the capturer gets their own.
        let ownerless = Project {
            project_id: "project-ownerless".to_owned(),
            repository: "legacy-repo".to_owned(),
            name: "Legacy".to_owned(),
            default_branch: None,
            created_at: "1".to_owned(),
            updated_at: "1".to_owned(),
        };
        store.state.projects.push(ownerless);
        capture_in(
            &mut store,
            &outsider.token,
            "x1",
            repo_context("legacy-repo", Some("main")),
        )?;
        assert_eq!(store.project_owner_id("project-ownerless"), None);
        let mine = store.projects(&outsider.token)?;
        assert!(mine.iter().any(|p| p.repository == "legacy-repo"
            && p.project_id != "project-ownerless"
            && store.project_owner_id(&p.project_id).as_deref()
                == Some(outsider.account.person_id.as_str())));
        assert!(
            store
                .state
                .project_branches
                .iter()
                .all(|b| b.project_id != "project-ownerless")
        );
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn starting_a_session_on_a_shared_branch_leaves_other_open_sessions_alone()
    -> Result<(), StoreError> {
        let path = temp_store_path("no-close-on-start");
        let mut store = MemoryStore::open(&path)?;
        let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
        let a_id = a.account.person_id.clone();
        let main = repo_context("cairn-demo", Some("main"));
        store.start_session_for_account(&a.token, "s1", main.clone())?;
        store.capture_for_account(&a.token, observation_for("s1", "evt-s1", "branchfact"))?;
        // A parallel agent on the same repository and branch keeps its fresh session.
        store.start_session_for_account(&a.token, "s2", main)?;
        assert!(!ended(&store, &a_id, "s1"));
        assert!(summaries(&store, "s1").is_empty());
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn a_summary_the_person_hid_is_never_derived_again() -> Result<(), StoreError> {
        let path = temp_store_path("hidden-summary");
        let mut store = MemoryStore::open(&path)?;
        let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
        for (session, how) in [("forgot", "forget"), ("fixed", "correct")] {
            store.start_session_for_account(&a.token, session, WorkContext::default())?;
            store.capture_for_account(
                &a.token,
                observation_for(session, &format!("{session}-1"), "hiddenfact"),
            )?;
            let summary = store.end_session_for_account(&a.token, session)?;
            if how == "forget" {
                store.forget_for_account(&a.token, &summary.memory_id)?;
            } else {
                store.correct_for_account(
                    &a.token,
                    &summary.memory_id,
                    "corrected text",
                    Vec::new(),
                    context(),
                    LifecycleInput::default(),
                )?;
            }
            store.capture_for_account(
                &a.token,
                observation_for(session, &format!("{session}-2"), "morefact"),
            )?;
            age_observations(&mut store, session, 31 * 60);
            store.capture_for_account(
                &a.token,
                observation_for("trigger", &format!("t-{session}"), "trigger"),
            )?;
            let kept = summaries(&store, session);
            assert_eq!(kept.len(), 1, "{how}: not replaced");
            assert_eq!(
                kept[0].source_event_ids.len(),
                1,
                "{how}: not derived again"
            );
            // An explicit end reports the hidden summary instead of returning it as success.
            let error = store
                .end_session_for_account(&a.token, session)
                .expect_err("hidden summary");
            assert!(
                error.to_string().contains("forgotten or superseded"),
                "{how}: {error}"
            );
        }
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn a_rederived_summary_keeps_verification_lifecycle_and_creation_time() -> Result<(), StoreError>
    {
        let path = temp_store_path("rederive-keeps-state");
        let mut store = MemoryStore::open(&path)?;
        let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
        store.start_session_for_account(&a.token, "keep", WorkContext::default())?;
        store.capture_for_account(&a.token, observation_for("keep", "keep-1", "keepfact"))?;
        store.end_session_for_account(&a.token, "keep")?;
        let id = summaries(&store, "keep")[0].memory_id.clone();
        for memory in &mut store.state.memories {
            if memory.memory_id == id {
                memory.verified_commit = Some("verified1".to_owned());
                memory.created_at = "1000".to_owned();
                memory.lifecycle.freshness = 42;
                memory.lifecycle.sensitivity = Sensitivity::Secret;
            }
        }
        store.capture_for_account(&a.token, observation_for("keep", "keep-2", "againfact"))?;
        let again = store.end_session_for_account(&a.token, "keep")?;
        assert_eq!(again.source_event_ids.len(), 2);
        assert_eq!(again.verified_commit.as_deref(), Some("verified1"));
        assert_eq!(again.created_at, "1000");
        assert_eq!(again.lifecycle.freshness, 42);
        assert_eq!(again.lifecycle.sensitivity, Sensitivity::Secret);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn a_summary_with_a_forgotten_source_is_derived_again_without_it() -> Result<(), StoreError> {
        let path = temp_store_path("forgotten-source");
        let mut store = MemoryStore::open(&path)?;
        let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
        store.start_session_for_account(&a.token, "src", WorkContext::default())?;
        store.capture_for_account(&a.token, observation_for("src", "src-1", "forgetmeword"))?;
        store.capture_for_account(&a.token, observation_for("src", "src-2", "keepmeword"))?;
        let first = store.end_session_for_account(&a.token, "src")?;
        assert!(first.content.contains("forgetmeword"));
        store.forget_for_account(&a.token, "src-1")?;
        let filter = RecallFilter::default();
        assert!(
            store
                .search_for_account(&a.token, "keepmeword", 5, &filter)?
                .is_empty(),
            "hidden with its source"
        );
        age_observations(&mut store, "src", 31 * 60);
        store.capture_for_account(&a.token, observation_for("trigger", "t-src", "trigger"))?;
        let hits = store.search_for_account(&a.token, "keepmeword", 5, &filter)?;
        assert_eq!(hits.len(), 1);
        assert!(!hits[0].content.contains("forgetmeword"));
        assert_eq!(hits[0].source_event_ids, vec!["src-2".to_owned()]);
        assert_eq!(summaries(&store, "src").len(), 1);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn auto_registration_validates_the_repository_length() -> Result<(), StoreError> {
        let path = temp_store_path("auto-register-length");
        let mut store = MemoryStore::open(&path)?;
        let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
        capture_in(
            &mut store,
            &a.token,
            "long-1",
            repo_context(&"r".repeat(513), Some("main")),
        )?;
        assert!(store.projects(&a.token)?.is_empty());
        assert!(store.state.project_branches.is_empty());
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn promoting_a_correction_does_not_supersede_a_forgotten_copy() -> Result<(), StoreError> {
        let path = temp_store_path("prior-copy-live");
        let mut store = MemoryStore::open(&path)?;
        let owner =
            store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
        let (project_id, copy) = owned_project(&mut store, &owner.token, "priorcopy fact")?;
        let source_id = copy.promoted_from.clone().expect("origin");
        store.forget_for_account(&owner.token, &copy.memory_id)?;
        let correction = store.correct_for_account(
            &owner.token,
            &source_id,
            "priorcopy corrected fact",
            Vec::new(),
            context(),
            LifecycleInput::default(),
        )?;
        let promoted =
            store.promote_memory(&owner.token, &project_id, &correction.memory_id, "why")?;
        assert_eq!(
            promoted.supersedes, None,
            "a forgotten copy is not a predecessor"
        );
        fs::remove_file(path)?;
        Ok(())
    }

    fn context_from_fixture(value: &Value) -> WorkContext {
        WorkContext {
            project_id: None,
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

    #[test]
    fn file_sync_push_registers_project_and_closes_idle_sessions() -> Result<(), StoreError> {
        let path = temp_store_path("sync-housekeeping");
        let mut store = MemoryStore::open(&path)?;
        let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
        let a_id = a.account.person_id.clone();
        let device = store.register_device("dev-house", &a_id, None, "Hook")?;
        store.start_session_for_account(&device.token, "idle-push", WorkContext::default())?;
        let mut first = observation_for("idle-push", "evt-idle-push", "pushidle");
        first.context = repo_context("push-repo", Some("feat/push"));
        store.sync_push(&device.token, first)?;
        let projects = store.projects(&a.token)?;
        assert_eq!(projects.len(), 1, "push registers the project");
        assert_eq!(projects[0].repository, "push-repo");
        let branches = store.branches(&a.token, &projects[0].project_id)?;
        assert_eq!(branches.len(), 1);
        assert_eq!(branches[0].name, "feat/push");
        age_observations(&mut store, "idle-push", 31 * 60);
        assert!(!ended(&store, &a_id, "idle-push"));

        store.sync_push(
            &device.token,
            observation_for("trigger", "evt-trigger", "trigger"),
        )?;

        assert!(
            ended(&store, &a_id, "idle-push"),
            "push sweeps idle sessions"
        );
        assert_eq!(summaries(&store, "idle-push").len(), 1);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn equal_scores_rank_decisions_before_other_memories() -> Result<(), StoreError> {
        let path = temp_store_path("decision-first");
        let mut store = MemoryStore::open(&path)?;
        let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
        let filter = RecallFilter::default();
        let mut decision = String::new();
        for (label, kind) in [
            ("one", MemoryKind::Explicit),
            ("two", MemoryKind::Explicit),
            ("three", MemoryKind::Explicit),
            ("four", MemoryKind::Lesson),
            ("five", MemoryKind::Decision),
            ("six", MemoryKind::Explicit),
        ] {
            let memory = store.remember_for_account(
                &a.token,
                &format!("rankdecision {label}"),
                Vec::new(),
                WorkContext::default(),
                LifecycleInput::default(),
                kind.clone(),
            )?;
            if kind == MemoryKind::Decision {
                decision = memory.memory_id;
            }
        }
        // A strictly higher score still ranks first: scores are unchanged.
        let strong = remember_owned(&mut store, &a.token, "rankdecision rankdecision strong")?;

        let hits = store.search_for_account(&a.token, "rankdecision", 10, &filter)?;
        assert_eq!(hits[0].memory_id, strong.memory_id);
        assert_eq!(hits[1].memory_id, decision);
        let pack = store.context_for_account(&a.token, "rankdecision", 10, &filter)?;
        assert_eq!(pack.results[1].memory_id, decision);
        let plain = store.search("rankdecision", 10)?;
        assert_eq!(plain[1].memory_id, decision);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn only_the_admin_web_session_creates_accounts() -> Result<(), StoreError> {
        let path = temp_store_path("admin-register");
        let mut store = MemoryStore::open(&path)?;
        let pw = "correct horse battery staple";
        let admin = store.bootstrap_account("admin@example.test", pw)?;
        assert!(admin.account.is_admin);
        assert!(store.authenticate_account(&admin.token)?.is_admin);

        let member = store.register_account_as_admin(&admin.token, "m@example.test", pw)?;
        assert!(!member.account.is_admin);
        assert!(!store.login("m@example.test", pw)?.account.is_admin);
        assert!(!store.authenticate_account(&member.token)?.is_admin);

        let denied = |error: StoreError, text: &str| error.to_string().contains(text);
        let error = store
            .register_account_as_admin(&member.token, "x@example.test", pw)
            .unwrap_err();
        assert!(denied(error, ADMIN_ONLY_ERROR));
        let device = store.register_device("dev-admin", &admin.account.person_id, None, "Hook")?;
        let error = store
            .register_account_as_admin(&device.token, "x@example.test", pw)
            .unwrap_err();
        assert!(denied(error, WEB_ONLY_ERROR));
        let error = store
            .register_account_as_admin("nope", "x@example.test", pw)
            .unwrap_err();
        assert!(denied(error, "account authentication failed"));
        assert!(
            store.login("x@example.test", pw).is_err(),
            "nothing was created"
        );

        // The flag survives a reload.
        let reopened = MemoryStore::open(&path)?;
        assert!(reopened.authenticate_account(&admin.token)?.is_admin);
        assert!(!reopened.authenticate_account(&member.token)?.is_admin);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn periodic_sweep_closes_idle_sessions_for_every_person() -> Result<(), StoreError> {
        let path = temp_store_path("sweep-all");
        let mut store = MemoryStore::open(&path)?;
        let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
        let b = store.register_account("b@example.test", "correct horse battery staple")?;
        let (a_id, b_id) = (a.account.person_id.clone(), b.account.person_id.clone());
        for (token, session) in [
            (&a.token, "sweep-a"),
            (&a.token, "fresh-a"),
            (&b.token, "sweep-b"),
        ] {
            store.start_session_for_account(token, session, WorkContext::default())?;
            store.capture_for_account(
                token,
                observation_for(session, &format!("evt-{session}"), "sweepfact"),
            )?;
        }
        age_observations(&mut store, "sweep-a", 31 * 60);
        age_observations(&mut store, "sweep-b", 31 * 60);
        age_observations(&mut store, "fresh-a", 29 * 60);

        store.sweep_idle_sessions();

        assert!(ended(&store, &a_id, "sweep-a") && ended(&store, &b_id, "sweep-b"));
        assert!(!ended(&store, &a_id, "fresh-a"), "29 minutes is not idle");
        assert_eq!(summaries(&store, "sweep-a").len(), 1);
        assert_eq!(summaries(&store, "sweep-b").len(), 1);
        store.sweep_idle_sessions();
        assert_eq!(summaries(&store, "sweep-a").len(), 1, "no second summary");
        let reopened = MemoryStore::open(&path)?;
        assert!(ended(&reopened, &b_id, "sweep-b"));
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn hook_only_push_creates_the_session_and_the_sweep_summarises_it() -> Result<(), StoreError> {
        let path = temp_store_path("push-only-session");
        let mut store = MemoryStore::open(&path)?;
        let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
        let a_id = a.account.person_id.clone();
        let device = store.register_device("dev-hook", &a_id, None, "Hook")?;
        store.sync_push(
            &device.token,
            observation_for("hook-only", "evt-hook-only", "hookonlyfact"),
        )?;
        assert!(!ended(&store, &a_id, "hook-only"));
        age_observations(&mut store, "hook-only", 31 * 60);

        store.sweep_idle_sessions();

        assert!(ended(&store, &a_id, "hook-only"));
        assert_eq!(summaries(&store, "hook-only").len(), 1);
        // The same push replayed does not add a second session row.
        store.sync_push(
            &device.token,
            observation_for("hook-only", "evt-hook-only", "hookonlyfact"),
        )?;
        let rows = store
            .state
            .sessions
            .iter()
            .filter(|session| session.session_id == "hook-only")
            .count();
        assert_eq!(rows, 1);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn a_push_gives_the_session_its_context_so_the_summary_is_repository_scoped()
    -> Result<(), StoreError> {
        let path = temp_store_path("push-context-scope");
        let mut store = MemoryStore::open(&path)?;
        let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
        let device = store.register_device("dev-pc", &a.account.person_id, None, "Hook")?;
        let hook_context = |branch: &str| WorkContext {
            repository: Some("cairn-demo".to_owned()),
            branch: Some(branch.to_owned()),
            ..WorkContext::default()
        };
        let mut push = |session: &str, event: &str, context: WorkContext| {
            let mut input = observation_for(session, event, "pushscopefact");
            input.context = context;
            store.sync_push(&device.token, input)
        };
        push("created", "evt-created", hook_context("feat/hook"))?;
        // A push into an empty row fills it; a later push never replaces a real context.
        push("filled", "evt-filled-1", WorkContext::default())?;
        push("filled", "evt-filled-2", hook_context("feat/hook"))?;
        push("filled", "evt-filled-3", hook_context("feat/other"))?;
        for session in ["created", "filled"] {
            age_observations(&mut store, session, 31 * 60);
        }

        store.sweep_idle_sessions();

        for session in ["created", "filled"] {
            let summary = summaries(&store, session).remove(0);
            assert_eq!(summary.scope, MemoryScope::Branch, "{session}");
            assert_eq!(summary.context.branch.as_deref(), Some("feat/hook"));
        }
        let filter = RecallFilter {
            context: Some(hook_context("feat/hook")),
            ..RecallFilter::default()
        };
        let hits = store.search_with_filter("pushscopefact", 10, &filter)?;
        assert!(
            hits.iter()
                .any(|hit| hit.memory_id == summaries(&store, "created")[0].memory_id),
            "a repository/branch-filtered recall finds the hook-only summary"
        );
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn a_start_after_a_push_fills_the_empty_session_context() -> Result<(), StoreError> {
        let path = temp_store_path("push-then-start");
        let mut store = MemoryStore::open(&path)?;
        let a = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
        let device = store.register_device("dev-ps", &a.account.person_id, None, "Hook")?;
        let mut input = observation_for("push-start", "evt-push-start", "pushstartfact");
        input.context = WorkContext::default();
        store.sync_push(&device.token, input)?;
        let started = store.start_session_for_account(&a.token, "push-start", context())?;
        assert_eq!(started.context.branch.as_deref(), Some("feat/auth"));
        // A real context is kept: the next start does not replace it.
        let again =
            store.start_session_for_account(&a.token, "push-start", WorkContext::default())?;
        assert_eq!(again.context.branch.as_deref(), Some("feat/auth"));
        fs::remove_file(path)?;
        Ok(())
    }

    fn with_project_id(mut context: WorkContext, project_id: &str) -> WorkContext {
        context.project_id = Some(project_id.to_owned());
        context
    }

    #[test]
    fn a_capture_with_a_project_id_makes_the_capturer_a_member_once() -> Result<(), StoreError> {
        let path = temp_store_path("join-by-project-file");
        let mut store = MemoryStore::open(&path)?;
        let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
        let member = store.register_account("m@example.test", "correct horse battery staple")?;
        let shared = store.register_project(&owner.token, "cairn-demo", "Shared", None)?;
        store.create_project_team(&owner.token, &shared.project_id, "Core")?;

        // The member's repository name differs, so only the project id can link it.
        let context = with_project_id(
            repo_context("other-name", Some("feat/m")),
            &shared.project_id,
        );
        capture_in(&mut store, &member.token, "j1", context.clone())?;
        capture_in(&mut store, &member.token, "j2", context)?;

        assert_eq!(
            store.state.projects.len(),
            1,
            "no duplicate private project"
        );
        assert!(store.state.project_branches.is_empty());
        let projects = store.projects(&member.token)?;
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].project_id, shared.project_id);
        let team = store.project_team(&owner.token, &shared.project_id)?;
        assert_eq!(
            team.members.len(),
            1,
            "joining twice records one membership"
        );
        assert_eq!(team.members[0].email, "m@example.test");
        let stored = store
            .observations()
            .find(|observation| observation.event_id == "j1")
            .expect("the observation is stored");
        assert_eq!(
            stored.context.project_id.as_deref(),
            Some(shared.project_id.as_str())
        );
        let audit = store.audit_events(&owner.token, 10)?;
        assert_eq!(
            audit
                .iter()
                .filter(|event| event.action == "project_member_joined")
                .count(),
            1
        );

        // The new member can be shared with, and reads the shared decision.
        let source = remember_with_context(&mut store, &owner.token, "member shared source")?;
        let promoted =
            store.promote_memory(&owner.token, &shared.project_id, &source.memory_id, "why")?;
        store.share_project_memory(
            &owner.token,
            &shared.project_id,
            &promoted.memory_id,
            "m@example.test",
        )?;
        assert_eq!(
            store
                .shared_project_memories(&member.token, &shared.project_id)?
                .len(),
            1
        );
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn an_unknown_ownerless_or_own_project_id_is_ignored_by_a_capture() -> Result<(), StoreError> {
        let path = temp_store_path("join-ignored");
        let mut store = MemoryStore::open(&path)?;
        let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
        let other = store.register_account("x@example.test", "correct horse battery staple")?;
        let own = store.register_project(&owner.token, "own-repo", "Own", None)?;

        // Unknown id: the capture succeeds and registers the repository as before.
        capture_in(
            &mut store,
            &other.token,
            "u1",
            with_project_id(repo_context("fallback-repo", Some("main")), "project-nope"),
        )?;
        let mine = store.projects(&other.token)?;
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].repository, "fallback-repo");

        // A project with no owner record is never joined.
        store.state.projects.push(Project {
            project_id: "project-ownerless".to_owned(),
            repository: "legacy-repo".to_owned(),
            name: "Legacy".to_owned(),
            default_branch: None,
            created_at: "1".to_owned(),
            updated_at: "1".to_owned(),
        });
        capture_in(
            &mut store,
            &other.token,
            "u2",
            with_project_id(repo_context("legacy-repo", None), "project-ownerless"),
        )?;
        assert!(store.state.project_members.is_empty());

        // The owner's own project id adds no membership and still registers the branch.
        capture_in(
            &mut store,
            &owner.token,
            "u3",
            with_project_id(repo_context("own-repo", Some("feat/o")), &own.project_id),
        )?;
        assert!(store.state.project_members.is_empty());
        assert_eq!(store.branches(&owner.token, &own.project_id)?.len(), 1);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn a_device_sync_push_with_a_project_id_joins_and_a_bare_id_keeps_session_context()
    -> Result<(), StoreError> {
        let path = temp_store_path("join-by-push");
        let mut store = MemoryStore::open(&path)?;
        let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
        let member = store.register_account("m@example.test", "correct horse battery staple")?;
        let device = store.register_device("dev-m", &member.account.person_id, None, "Hook")?;
        let shared = store.register_project(&owner.token, "shared-repo", "Shared", None)?;

        store.start_session_for_account(&member.token, "bare", context())?;
        let mut input = observation_for("bare", "bare-1", "barefact");
        input.context = WorkContext {
            project_id: Some(shared.project_id.clone()),
            ..WorkContext::default()
        };
        let pushed = store.sync_push(&device.token, input)?;
        assert_eq!(store.projects(&member.token)?.len(), 1);
        assert_eq!(store.state.projects.len(), 1);
        assert_eq!(
            pushed.observation.context.project_id.as_deref(),
            Some(shared.project_id.as_str())
        );
        assert_eq!(
            pushed.observation.context.repository.as_deref(),
            Some("cairn-demo")
        );
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn init_project_creates_once_defaults_the_name_and_is_per_owner() -> Result<(), StoreError> {
        let path = temp_store_path("init-project");
        let mut store = MemoryStore::open(&path)?;
        let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
        let other = store.register_account("x@example.test", "correct horse battery staple")?;
        let (created, is_new) = store.init_project(&owner.token, " repo-a ", None)?;
        assert!(is_new);
        assert_eq!(
            (created.repository.as_str(), created.name.as_str()),
            ("repo-a", "repo-a")
        );
        let (again, is_new) = store.init_project(&owner.token, "repo-a", Some("Renamed"))?;
        assert!(!is_new);
        assert_eq!(again.project_id, created.project_id);
        assert_eq!(again.name, "repo-a");
        let (theirs, is_new) = store.init_project(&other.token, "repo-a", Some("Named"))?;
        assert!(is_new);
        assert_ne!(theirs.project_id, created.project_id);
        assert_eq!(theirs.name, "Named");
        assert!(store.init_project(&owner.token, "  ", None).is_err());
        assert!(
            store
                .init_project(&owner.token, &"r".repeat(513), None)
                .is_err()
        );
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn only_the_admin_session_lists_accounts() -> Result<(), StoreError> {
        let path = temp_store_path("list-accounts");
        let mut store = MemoryStore::open(&path)?;
        let admin = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
        let user = store.register_account("u@example.test", "correct horse battery staple")?;
        let device = store.register_device("dev-a", &admin.account.person_id, None, "Laptop")?;

        let listed = store.accounts(&admin.token)?;
        assert_eq!(
            listed.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(),
            ["a@example.test", "u@example.test"]
        );
        assert!(listed[0].is_admin && !listed[1].is_admin);
        let refused = store.accounts(&user.token).expect_err("not admin");
        assert!(refused.to_string().contains(ADMIN_ONLY_ERROR));
        let refused = store.accounts(&device.token).expect_err("device token");
        assert!(refused.to_string().contains(WEB_ONLY_ERROR));
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn the_admin_resets_a_password_and_every_web_session_of_that_user_ends()
    -> Result<(), StoreError> {
        let path = temp_store_path("reset-password");
        let mut store = MemoryStore::open(&path)?;
        let admin = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
        let user = store.register_account("u@example.test", "correct horse battery staple")?;
        let second = store.login("u@example.test", "correct horse battery staple")?;
        let device = store.register_device("dev-u", &user.account.person_id, None, "Laptop")?;
        let fresh = "a brand new passphrase";

        for (token, expected) in [
            (&user.token, ADMIN_ONLY_ERROR),
            (&device.token, WEB_ONLY_ERROR),
        ] {
            let refused = store
                .reset_account_password(token, &user.account.person_id, fresh)
                .expect_err("admin web session only");
            assert!(refused.to_string().contains(expected));
        }
        assert!(
            store
                .reset_account_password(&admin.token, &user.account.person_id, "short")
                .is_err()
        );
        assert!(
            store
                .reset_account_password(&admin.token, "person-missing", fresh)
                .is_err()
        );
        store.authenticate_session(&user.token)?;

        store.reset_account_password(&admin.token, &user.account.person_id, fresh)?;
        for token in [&user.token, &second.token] {
            assert!(store.authenticate_session(token).is_err());
        }
        assert!(
            store.authenticate_account(&device.token).is_ok(),
            "devices stay valid"
        );
        assert!(
            store
                .login("u@example.test", "correct horse battery staple")
                .is_err()
        );
        let relogin = store.login("u@example.test", fresh)?;
        store.authenticate_session(&admin.token)?;
        assert!(
            store
                .audit_events(&relogin.token, 10)?
                .iter()
                .any(|event| event.action == "account_password_reset")
        );
        let on_disk = fs::read_to_string(&path)?;
        assert!(!on_disk.contains(fresh));
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn changing_the_password_needs_the_current_one_and_keeps_only_this_session()
    -> Result<(), StoreError> {
        let path = temp_store_path("change-password");
        let mut store = MemoryStore::open(&path)?;
        let user = store.bootstrap_account("a@example.test", "correct horse battery staple")?;
        let other = store.login("a@example.test", "correct horse battery staple")?;
        let device = store.register_device("dev-a", &user.account.person_id, None, "Laptop")?;
        let fresh = "a brand new passphrase";

        let wrong = store
            .change_password(&user.token, "not the password", fresh)
            .expect_err("wrong current password");
        assert!(wrong.to_string().contains("invalid account credentials"));
        assert!(
            store
                .change_password(&user.token, "correct horse battery staple", "short")
                .is_err()
        );
        let refused = store
            .change_password(&device.token, "correct horse battery staple", fresh)
            .expect_err("device token");
        assert!(refused.to_string().contains(WEB_ONLY_ERROR));
        store.authenticate_session(&other.token)?;

        store.change_password(&user.token, "correct horse battery staple", fresh)?;
        store.authenticate_session(&user.token)?;
        assert!(store.authenticate_session(&other.token).is_err());
        assert!(store.authenticate_account(&device.token).is_ok());
        assert!(
            store
                .login("a@example.test", "correct horse battery staple")
                .is_err()
        );
        store.login("a@example.test", fresh)?;
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn project_people_lists_the_owner_then_members_for_owner_and_members_only()
    -> Result<(), StoreError> {
        let path = temp_store_path("project-people");
        let mut store = MemoryStore::open(&path)?;
        let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
        let member = store.register_account("m@example.test", "correct horse battery staple")?;
        let outsider = store.register_account("x@example.test", "correct horse battery staple")?;
        let (project, _) = store.init_project(&owner.token, "repo-p", None)?;

        // No team space is needed, and a fresh project has only its owner.
        let alone = store.project_people(&owner.token, &project.project_id)?;
        assert_eq!(alone.len(), 1);
        assert_eq!(
            (alone[0].role.as_str(), alone[0].email.as_str()),
            ("owner", "o@example.test")
        );
        assert_eq!(alone[0].person_id, owner.account.person_id);
        assert_eq!(alone[0].joined_at, project.created_at);

        join_project(&mut store, &member.token, &project.project_id)?;
        for token in [&owner.token, &member.token] {
            let people = store.project_people(token, &project.project_id)?;
            assert_eq!(
                people
                    .iter()
                    .map(|p| (p.role.as_str(), p.email.as_str()))
                    .collect::<Vec<_>>(),
                [("owner", "o@example.test"), ("member", "m@example.test")]
            );
        }
        let refused = store
            .project_people(&outsider.token, &project.project_id)
            .expect_err("outsider");
        assert!(refused.to_string().contains("not a project member"));
        assert!(
            store
                .project_people(&owner.token, "project-missing")
                .is_err()
        );
        fs::remove_file(path)?;
        Ok(())
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
