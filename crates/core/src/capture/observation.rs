use super::redact::{redact_sensitive_content, redact_work_context};
use crate::error::Error;
use crate::id::{now_seconds, sha256_hex};
use serde::{Deserialize, Serialize};

/// Largest accepted request body for one observation.
pub const MAX_OBSERVATION_BYTES: usize = 64 * 1024;
/// Stored content is clipped to this many bytes (on a character boundary).
pub const OBSERVATION_CLIP_BYTES: usize = 2 * 1024;
pub const OBSERVATION_TTL_SECONDS: i64 = 30 * 86_400;

fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// The client's work context. Unknown keys are ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<super::ChatContext>,
    /// The committed `.cairn/project.json` project id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub repository: Option<String>,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub commit: Option<String>,
    #[serde(default)]
    pub files: Vec<String>,
    /// How many edits were outside the project root. A count only: the paths stay on the client.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub outside_files: u32,
    #[serde(default)]
    pub symbols: Vec<String>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub tests: Vec<String>,
    #[serde(default)]
    pub services: Vec<String>,
    /// `"automation"` for an automated or subagent session. `upsert_session` copies it onto
    /// `sessions.context`; it is not a git fact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

/// What the hook captured (apps/docs/content/technical/hooks/index.md#events).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureKind {
    Prompt,
    Edit,
    Command,
    Assistant,
}

impl CaptureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prompt => "prompt",
            Self::Edit => "edit",
            Self::Command => "command",
            Self::Assistant => "assistant",
        }
    }

    /// `user` for a prompt, `assistant` for the model's text, `tool` for everything else.
    pub fn role(self) -> &'static str {
        match self {
            Self::Prompt => "user",
            Self::Assistant => "assistant",
            Self::Edit | Self::Command => "tool",
        }
    }
}

impl std::str::FromStr for CaptureKind {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        [Self::Prompt, Self::Edit, Self::Command, Self::Assistant]
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .ok_or_else(|| format!("unknown capture kind: {text}"))
    }
}

/// Unknown keys are a 400. The store derives `role` and sets `source` to the kind.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationInput {
    pub event_id: String,
    pub session_id: String,
    pub kind: CaptureKind,
    #[serde(default)]
    pub ok: Option<bool>,
    pub content: String,
    #[serde(default)]
    pub context: WorkContext,
}

/// A raw capture as stored: redacted, clipped, never recalled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Observation {
    pub event_id: String,
    pub session_id: String,
    pub content: String,
    pub kind: CaptureKind,
    pub ok: Option<bool>,
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
        content_hash: sha256_hex(&[&input.session_id, input.kind.as_str(), &content]),
        event_id: input.event_id.clone(),
        session_id: input.session_id.clone(),
        content,
        kind: input.kind,
        ok: input.ok,
        role: input.kind.role().to_owned(),
        source: input.kind.as_str().to_owned(),
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
            "event_id": "e1", "session_id": "s1", "content": content, "kind": "command",
            "ok": false, "context": {"files": ["a.rs"], "handoffs": ["ignored"]}
        }))
        .expect("valid input")
    }

    #[test]
    fn unknown_keys_and_kinds_are_rejected_and_role_is_derived() {
        for extra in [("role", "user"), ("source", "hook"), ("kind", "tool")] {
            let mut value = serde_json::json!({
                "event_id": "e1", "session_id": "s1", "content": "x", "kind": "prompt"
            });
            value[extra.0] = extra.1.into();
            assert!(serde_json::from_value::<ObservationInput>(value).is_err());
        }
        let obs = normalize_observation(&input("ls")).expect("valid");
        assert_eq!(
            (obs.role.as_str(), obs.source.as_str()),
            ("tool", "command")
        );
        assert_eq!(obs.ok, Some(false));
        assert_eq!(CaptureKind::Prompt.role(), "user");
        assert_eq!(CaptureKind::Assistant.role(), "assistant");
    }

    #[test]
    fn same_text_with_another_kind_hashes_differently() {
        let mut other = input("ls");
        other.kind = CaptureKind::Prompt;
        assert_ne!(
            normalize_observation(&input("ls")).unwrap().content_hash,
            normalize_observation(&other).unwrap().content_hash
        );
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
