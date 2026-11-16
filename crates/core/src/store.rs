use crate::accounts::{AccountRecord, AuthSessionRecord, StoredAuditEvent};
use crate::capture::Observation;
use crate::devices::DeviceRecord;
use crate::memories::Memory;
use crate::ownership::{
    MemoryOwnerRecord, ObservationOwnerRecord, ProjectMemberRecord, ProjectOwnerRecord,
};
use crate::projects::{
    Project, ProjectBranch, ProjectEntity, ProjectFreshnessEvent, ProjectWorktree,
};
use crate::sessions::Session;
use crate::sync::{SyncDeletionRecord, SyncEvent};
use crate::types::StoreError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const STORE_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct StoreState {
    version: u32,
    #[serde(default)]
    pub(crate) sessions: Vec<Session>,
    #[serde(default)]
    pub(crate) observations: Vec<Observation>,
    #[serde(default)]
    pub(crate) memories: Vec<Memory>,
    #[serde(default)]
    pub(crate) forgotten_ids: HashSet<String>,
    #[serde(default)]
    pub(crate) devices: Vec<DeviceRecord>,
    #[serde(default)]
    pub(crate) sync_events: Vec<SyncEvent>,
    #[serde(default)]
    pub(crate) next_sync_cursor: u64,
    #[serde(default)]
    pub(crate) accounts: Vec<AccountRecord>,
    #[serde(default)]
    pub(crate) auth_sessions: Vec<AuthSessionRecord>,
    #[serde(default)]
    pub(crate) projects: Vec<Project>,
    #[serde(default)]
    pub(crate) project_branches: Vec<ProjectBranch>,
    #[serde(default)]
    pub(crate) project_worktrees: Vec<ProjectWorktree>,
    #[serde(default)]
    pub(crate) project_entities: Vec<ProjectEntity>,
    #[serde(default)]
    pub(crate) freshness_events: Vec<ProjectFreshnessEvent>,
    #[serde(default)]
    pub(crate) project_owners: Vec<ProjectOwnerRecord>,
    #[serde(default)]
    pub(crate) memory_owners: Vec<MemoryOwnerRecord>,
    #[serde(default)]
    pub(crate) observation_owners: Vec<ObservationOwnerRecord>,
    #[serde(default)]
    pub(crate) sync_deletions: Vec<SyncDeletionRecord>,
    #[serde(default)]
    pub(crate) project_members: Vec<ProjectMemberRecord>,
    #[serde(default)]
    pub(crate) audit_events: Vec<StoredAuditEvent>,
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
            audit_events: Vec::new(),
        }
    }
}

pub struct MemoryStore {
    path: PathBuf,
    pub(crate) state: StoreState,
    /// Last state known to be on disk. A failed `persist` restores it.
    pub(crate) persisted: StoreState,
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

    /// Writes the state. Every mutator commits through here, so a failed write restores the
    /// last persisted state and memory never runs ahead of the disk.
    pub(crate) fn persist(&mut self) -> Result<(), StoreError> {
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

impl MemoryStore {
    pub fn observations(&self) -> impl Iterator<Item = &Observation> {
        self.state.observations.iter()
    }

    pub fn sessions(&self) -> impl Iterator<Item = &Session> {
        self.state.sessions.iter()
    }
}
