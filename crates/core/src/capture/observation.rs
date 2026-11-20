use super::redact::{redact_sensitive_content, redact_work_context};
use crate::error::Error;
use crate::id::{now_seconds, sha256_hex};
use serde::{Deserialize, Serialize};

/// Largest accepted request body for one observation.
pub const MAX_OBSERVATION_BYTES: usize = 64 * 1024;
/// Stored content is clipped to this many bytes (on a character boundary).
pub const OBSERVATION_CLIP_BYTES: usize = 2 * 1024;
pub const OBSERVATION_TTL_SECONDS: i64 = 30 * 86_400;

/// The client's work context. Unknown keys from the Python hook are ignored until step 4.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkContext {
    /// The committed `.cairn/project.json` project id.
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
}

/// Unknown keys (the old `importance`, `lifecycle`) are accepted and ignored.
#[derive(Clone, Debug, Deserialize)]
pub struct ObservationInput {
    pub event_id: String,
    pub session_id: String,
    pub content: String,
    pub role: String,
    pub source: String,
    #[serde(default)]
    pub context: WorkContext,
}

/// A raw capture as stored: redacted, clipped, never recalled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Observation {
    pub event_id: String,
    pub session_id: String,
    pub content: String,
    pub role: String,
    pub source: String,
    /// Same event id with another hash is a 409.
    pub content_hash: String,
    pub context: WorkContext,
    pub captured_at: i64,
    pub expires_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CaptureResult {
    pub accepted: bool,
    pub duplicate: bool,
    pub event_id: String,
}

pub fn normalize_observation(input: &ObservationInput) -> Result<Observation, Error> {
    for (name, value) in [
        ("event_id", &input.event_id),
        ("session_id", &input.session_id),
        ("content", &input.content),
        ("role", &input.role),
        ("source", &input.source),
    ] {
        if value.trim().is_empty() {
            return Err(Error::validation(format!("{name} must not be empty")));
        }
    }
    if input.content.len() > MAX_OBSERVATION_BYTES {
        return Err(Error::validation(format!(
            "content exceeds the {MAX_OBSERVATION_BYTES} byte limit"
        )));
    }
    let content = clip(
        &redact_sensitive_content(&input.content),
        OBSERVATION_CLIP_BYTES,
    );
    let now = now_seconds();
    Ok(Observation {
        content_hash: sha256_hex(&[&input.session_id, &input.role, &input.source, &content]),
        event_id: input.event_id.clone(),
        session_id: input.session_id.clone(),
        content,
        role: input.role.clone(),
        source: input.source.clone(),
        context: redact_work_context(&input.context),
        captured_at: now,
        expires_at: now + OBSERVATION_TTL_SECONDS,
    })
}

fn clip(text: &str, max_bytes: usize) -> String {
    let mut end = text.len().min(max_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(content: &str) -> ObservationInput {
        serde_json::from_value(serde_json::json!({
            "event_id": "e1", "session_id": "s1", "content": content, "role": "user",
            "source": "hook", "importance": "high", "lifecycle": {"x": 1},
            "context": {"files": ["a.rs"], "handoffs": ["ignored"]}
        }))
        .expect("old hook keys are ignored")
    }

    #[test]
    fn redacts_clips_and_hashes() {
        let obs = normalize_observation(&input(&format!("password=hunter2 {}", "é".repeat(3000))))
            .expect("valid");
        assert!(!obs.content.contains("hunter2"));
        assert!(obs.content.len() <= OBSERVATION_CLIP_BYTES);
        assert_eq!(obs.expires_at - obs.captured_at, OBSERVATION_TTL_SECONDS);
        let other = normalize_observation(&input("different")).expect("valid");
        assert_ne!(obs.content_hash, other.content_hash);
    }

    #[test]
    fn rejects_blank_and_oversized() {
        assert!(normalize_observation(&input("  ")).is_err());
        let mut big = input("x");
        big.content = "x".repeat(MAX_OBSERVATION_BYTES + 1);
        assert!(normalize_observation(&big).is_err());
    }
}
