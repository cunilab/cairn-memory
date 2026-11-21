use serde::Serialize;
use std::collections::BTreeMap;

/// Actions Cairn takes on its own. Everything else in the audit log is a person or device action.
pub const SYSTEM_ACTIONS: [&str; 8] = [
    "memory_merged",
    "links_added",
    "memory_promoted",
    "memory_withdrawn",
    "repo_state",
    "memories_expired",
    "session_distilled",
    "job_quarantined",
];

/// Metadata only; feeds the web Activity page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AuditEvent {
    pub audit_id: String,
    pub action: String,
    pub subject_id: Option<String>,
    pub device_id: Option<String>,
    /// The device's name, for a device the person owns (a system action may name it in metadata).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    pub metadata: serde_json::Value,
    /// Titles of the memories this event names that the reader may still see.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub titles: BTreeMap<String, String>,
    pub created_at: i64,
}
