use serde::{Deserialize, Serialize};
use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

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

pub(crate) fn timestamp() -> String {
    now_seconds().to_string()
}

pub fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn timestamp_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
