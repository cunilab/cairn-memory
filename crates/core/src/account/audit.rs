use serde::Serialize;

/// Metadata only; feeds the web Activity page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AuditEvent {
    pub audit_id: String,
    pub action: String,
    pub subject_id: Option<String>,
    pub device_id: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: i64,
}
