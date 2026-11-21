use crate::capture::Observation;
use crate::id::sha256_hex;
use crate::memory::validate::{LIST_ITEM_MAX, LIST_MAX_ITEMS, SUMMARY_MAX, TITLE_MAX};
use crate::memory::{Kind, SaveInput, Scope};
use serde_json::{Map, Value, json};

const MAX_TYPED_NOTES: usize = 64;

/// Rule-based output of one session: typed notes from `Decision:`-style lines, and a handoff.
/// Both are plain `SaveInput`s, so they pass the same validation as an agent's save.
#[derive(Clone, Debug, Default)]
pub struct Distilled {
    pub notes: Vec<SaveInput>,
    pub handoff: Option<SaveInput>,
}

/// One handoff per (owner, session): the id is derived, so a later distill upserts it.
pub fn handoff_id(person_id: &str, session_id: &str) -> String {
    format!("hnd_{}", &sha256_hex(&[person_id, session_id])[..32])
}

pub fn distill(observations: &[Observation]) -> Distilled {
    let usable: Vec<&Observation> = observations
        .iter()
        .filter(|o| !is_tool_payload(o))
        .collect();
    let session_id = observations.first().map_or("", |o| o.session_id.as_str());
    Distilled {
        notes: typed_notes(&usable),
        handoff: handoff(&usable, session_id),
    }
}

fn handoff(observations: &[&Observation], session_id: &str) -> Option<SaveInput> {
    let mut next_steps = Vec::new();
    let mut open = Vec::new();
    let mut files = Vec::new();
    let mut evidence = Vec::new();
    for observation in observations {
        for file in &observation.context.files {
            push_unique(&mut files, file);
        }
        for line in observation.content.lines() {
            let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
            let lower = line.to_ascii_lowercase();
            let mut used = false;
            if has_marker(
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
                used |= push_unique(&mut next_steps, &line);
            }
            if has_marker(
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
                used |= push_unique(&mut open, &line);
            }
            if used {
                push_unique(&mut evidence, &observation.event_id);
            }
        }
    }
    if next_steps.is_empty() && open.is_empty() {
        return None;
    }
    Some(SaveInput {
        kind: Kind::Handoff,
        title: clip(&format!("Handoff {session_id}"), TITLE_MAX),
        summary: clip(
            &format!(
                "{} next steps; {} open items recorded in session captures.",
                next_steps.len(),
                open.len()
            ),
            SUMMARY_MAX,
        ),
        reason: "Derived from the captures of one session.".to_owned(),
        fields: Some(json!({ "next_steps": next_steps, "open_questions": open, "files": files })),
        evidence,
        supersedes: None,
        contradicts: None,
        scope: Some(Scope::Personal),
    })
}

fn typed_notes(observations: &[&Observation]) -> Vec<SaveInput> {
    let mut notes: Vec<Draft> = Vec::new();
    'sources: for observation in observations {
        let mut current: Option<Draft> = None;
        for line in observation.content.lines() {
            let Some((label, value)) = line.trim().split_once(':') else {
                notes.extend(current.take());
                continue;
            };
            let label = label.trim().to_ascii_lowercase();
            let value = value.trim();
            if let Some(kind) = typed_kind(&label) {
                notes.extend(current.take());
                if notes.len() >= MAX_TYPED_NOTES {
                    break 'sources;
                }
                if !value.is_empty() {
                    current = Some(Draft::new(kind, value, &observation.event_id));
                }
            } else if let Some(draft) = current.as_mut()
                && draft.take_field(&label, value)
            {
                // Metadata must immediately follow its typed note; prose ends it.
            } else {
                notes.extend(current.take());
            }
        }
        notes.extend(current);
        if notes.len() >= MAX_TYPED_NOTES {
            break;
        }
    }
    notes.into_iter().map(Draft::into_input).collect()
}

fn typed_kind(label: &str) -> Option<Kind> {
    match label {
        "decision" => Some(Kind::Decision),
        "gotcha" => Some(Kind::Gotcha),
        "procedure" => Some(Kind::Procedure),
        "rule" => Some(Kind::Rule),
        "fact" => Some(Kind::Fact),
        _ => None,
    }
}

struct Draft {
    kind: Kind,
    value: String,
    reason: String,
    fields: Map<String, Value>,
    event_id: String,
}

impl Draft {
    fn new(kind: Kind, value: &str, event_id: &str) -> Self {
        Self {
            kind,
            value: value.to_owned(),
            reason: String::new(),
            fields: Map::new(),
            event_id: event_id.to_owned(),
        }
    }

    /// Maps a metadata label to this kind's field. False when the label does not belong.
    fn take_field(&mut self, label: &str, value: &str) -> bool {
        if value.is_empty() {
            return false;
        }
        let key = match (self.kind, label) {
            (_, "reason") => {
                self.reason = value.to_owned();
                return true;
            }
            (Kind::Decision, "decision context") => "context",
            (Kind::Decision, "consequences") => "consequences",
            (Kind::Decision, "alternative" | "alternatives") => {
                return self.push("alternatives", value);
            }
            (Kind::Rule, "exceptions") => "exceptions",
            (Kind::Gotcha, "symptom") => "symptom",
            (Kind::Gotcha, "cause") => "cause",
            (Kind::Gotcha, "fix") => "fix",
            (Kind::Procedure, "step") => return self.push("steps", value),
            (Kind::Procedure, "when") => "when",
            (Kind::Fact, "source") => "source",
            _ => return false,
        };
        self.fields
            .insert(key.to_owned(), Value::String(value.to_owned()));
        true
    }

    fn push(&mut self, key: &str, value: &str) -> bool {
        let list = self.fields.entry(key).or_insert_with(|| json!([]));
        if let Value::Array(items) = list
            && items.len() < LIST_MAX_ITEMS
        {
            items.push(Value::String(clip(value, LIST_ITEM_MAX)));
        }
        true
    }

    fn into_input(self) -> SaveInput {
        SaveInput {
            kind: self.kind,
            title: clip(&self.value, TITLE_MAX),
            summary: clip(&self.value, SUMMARY_MAX),
            // Provenance only; no cause is invented.
            reason: if self.reason.is_empty() {
                "Distilled from a captured session note.".to_owned()
            } else {
                self.reason
            },
            fields: Some(Value::Object(self.fields)),
            evidence: vec![self.event_id],
            supersedes: None,
            contradicts: None,
            scope: Some(Scope::Personal),
        }
    }
}

fn clip(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

fn has_marker(lower: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| lower.contains(marker))
}

fn push_unique(list: &mut Vec<String>, value: &str) -> bool {
    let value = clip(value, LIST_ITEM_MAX);
    if value.is_empty() || list.len() >= LIST_MAX_ITEMS || list.contains(&value) {
        return false;
    }
    list.push(value);
    true
}

/// Tool output stays an observation; it never becomes a note or a next step.
fn is_tool_payload(observation: &Observation) -> bool {
    let role = observation.role.trim().to_ascii_lowercase();
    let source = observation
        .source
        .trim()
        .to_ascii_lowercase()
        .replace(['_', '-'], "");
    if matches!(source.as_str(), "posttooluse" | "pretooluse")
        && role != "user"
        && role != "assistant"
    {
        return true;
    }
    let body = observation.content.trim();
    role == "tool"
        && ((body.starts_with('{') && body.ends_with('}'))
            || (body.starts_with('[') && body.ends_with(']')))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::WorkContext;

    fn obs(id: &str, role: &str, source: &str, content: &str) -> Observation {
        Observation {
            event_id: id.into(),
            session_id: "s1".into(),
            content: content.into(),
            role: role.into(),
            source: source.into(),
            kind: crate::capture::CaptureKind::Command,
            ok: None,
            content_hash: String::new(),
            context: WorkContext {
                files: vec!["a.rs".into()],
                ..WorkContext::default()
            },
            captured_at: 0,
            expires_at: 0,
        }
    }

    #[test]
    fn typed_notes_take_following_metadata_only() {
        let o = obs(
            "e1",
            "assistant",
            "hook",
            "Decision: use pgvector\nReason: one database\nAlternative: qdrant\nSome prose\nRule: no mocks\nFix: ignored for a rule",
        );
        let d = distill(&[o]);
        assert_eq!(d.notes.len(), 2);
        let decision = &d.notes[0];
        assert_eq!(
            (decision.kind, decision.reason.as_str()),
            (Kind::Decision, "one database")
        );
        assert_eq!(
            decision.fields.as_ref().unwrap()["alternatives"][0],
            "qdrant"
        );
        assert_eq!(decision.evidence, vec!["e1".to_owned()]);
        assert_eq!(decision.scope, Some(Scope::Personal));
        assert!(d.notes[1].reason.starts_with("Distilled"));
        assert!(
            d.notes[1]
                .fields
                .as_ref()
                .unwrap()
                .as_object()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn handoff_collects_next_steps_and_skips_tool_payloads() {
        let d = distill(&[
            obs("e1", "user", "hook", "TODO: wire the worker\nall done here"),
            obs("e2", "tool", "hook", "{\"todo\": \"from a tool\"}"),
        ]);
        let handoff = d.handoff.expect("handoff");
        assert_eq!(handoff.kind, Kind::Handoff);
        let fields = handoff.fields.unwrap();
        assert_eq!(fields["next_steps"][0], "TODO: wire the worker");
        assert_eq!(fields["files"][0], "a.rs");
        assert_eq!(handoff.evidence, vec!["e1".to_owned()]);
    }

    #[test]
    fn nothing_to_hand_off_gives_none() {
        assert!(
            distill(&[obs("e1", "user", "hook", "hello")])
                .handoff
                .is_none()
        );
        assert_eq!(handoff_id("p", "s"), handoff_id("p", "s"));
        assert_ne!(handoff_id("p", "s"), handoff_id("p", "t"));
    }
}
