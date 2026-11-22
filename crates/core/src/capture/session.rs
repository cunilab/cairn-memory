use serde::Serialize;

/// One host-agent session. `pending_since` is set while captures wait to be distilled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Session {
    pub person_id: String,
    pub session_id: String,
    pub project_id: Option<String>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub pending_since: Option<i64>,
}

/// A session is idle when it has pending captures and nothing arrived for this long.
pub const IDLE_SESSION_SECONDS: i64 = 30 * 60;

impl Session {
    pub fn is_idle(&self, last_capture_at: i64, now: i64) -> bool {
        self.ended_at.is_none()
            && self.pending_since.is_some()
            && now - last_capture_at >= IDLE_SESSION_SECONDS
    }
}
