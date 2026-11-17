use crate::{Memory, MemoryFields, MemoryKind, Observation, redact_memory_metadata};
use sha2::{Digest, Sha256};

const MAX_TYPED_NOTES: usize = 64;

/// Declared links stay on the summary. A derived note is a different memory.
fn note_from_summary(summary: &Memory) -> Memory {
    let mut note = summary.clone();
    note.contradicts.clear();
    note.supersedes = None;
    note
}

/// Derive bounded, evidence-linked work notes. Scope is inherited from the checked
/// session summary so mixed or unattached captures cannot broaden visibility.
pub fn distill_session_notes(summary: &Memory, observations: &[Observation]) -> Vec<Memory> {
    let mut handoff = note_from_summary(summary);
    handoff.memory_id = format!("handoff-{}", summary.memory_id);
    handoff.kind = MemoryKind::Handoff;
    handoff.fields.title = format!("Handoff {}", summary.session_id);
    handoff.fields.summary = format!(
        "{} next steps; {} unresolved items recorded in source captures.",
        summary.next_steps.len(),
        summary.unresolved_work.len()
    );
    handoff.content = format!(
        "Next steps:\n{}\nUnresolved work:\n{}",
        summary.next_steps.join("\n"),
        summary.unresolved_work.join("\n")
    );
    handoff.fields.details = Some(handoff.content.clone());
    let mut notes = vec![handoff];
    'sources: for observation in observations {
        let mut current: Option<Memory> = None;
        for (line_number, line) in observation.content.lines().enumerate() {
            let Some((label, value)) = line.trim().split_once(':') else {
                if let Some(note) = current.take() {
                    notes.push(note);
                }
                continue;
            };
            let label = label.trim().to_ascii_lowercase();
            let value = value.trim();
            let kind = match label.as_str() {
                "decision" => Some(MemoryKind::Decision),
                "gotcha" => Some(MemoryKind::Gotcha),
                "procedure" => Some(MemoryKind::Procedure),
                "rule" => Some(MemoryKind::Rule),
                "fact" => Some(MemoryKind::Fact),
                _ => None,
            };
            if let Some(kind) = kind {
                if let Some(note) = current.take() {
                    notes.push(note);
                }
                if notes.len() > MAX_TYPED_NOTES {
                    break 'sources;
                }
                if value.is_empty() {
                    continue;
                }
                let mut note = note_from_summary(summary);
                let mut identity = Sha256::new();
                for part in [&summary.memory_id, &observation.event_id] {
                    identity.update((part.len() as u64).to_le_bytes());
                    identity.update(part.as_bytes());
                }
                identity.update((line_number as u64).to_le_bytes());
                note.memory_id = format!("distilled-{:x}", identity.finalize());
                note.kind = kind;
                note.content = line.trim().to_owned();
                note.fields = MemoryFields {
                    title: value.chars().take(120).collect(),
                    summary: value.to_owned(),
                    details: Some(note.content.clone()),
                    author: summary.fields.author.clone(),
                    importance: summary.fields.importance,
                    applies_to: summary.fields.applies_to.clone(),
                    evidence: vec![observation.event_id.clone()],
                    tags: vec!["work-note".to_owned(), "distilled".to_owned(), label],
                    ..MemoryFields::default()
                };
                note.source_event_ids = vec![observation.event_id.clone()];
                note.next_steps.clear();
                note.unresolved_work.clear();
                current = Some(note);
            } else if let Some(note) = current.as_mut()
                && explicit_field(&mut note.fields, &note.kind, &label, value)
            {
                note.content.push('\n');
                note.content.push_str(line.trim());
                note.fields.details = Some(note.content.clone());
            } else {
                // Metadata must immediately follow its typed note; ordinary prose ends it.
                if let Some(note) = current.take() {
                    notes.push(note);
                }
            }
        }
        if let Some(note) = current {
            notes.push(note);
        }
        if notes.len() > MAX_TYPED_NOTES {
            break;
        }
    }
    for note in &mut notes {
        redact_memory_metadata(note);
    }
    notes
}

fn explicit_field(fields: &mut MemoryFields, kind: &MemoryKind, label: &str, value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let target = match (kind, label) {
        (_, "reason") => {
            fields.reason = value.to_owned();
            return true;
        }
        (MemoryKind::Decision, "decision context") => &mut fields.decision_context,
        (MemoryKind::Decision, "consequences") => &mut fields.consequences,
        (MemoryKind::Decision, "alternative" | "alternatives") => {
            fields.alternatives.push(value.to_owned());
            return true;
        }
        (MemoryKind::Rule, "rule text") => &mut fields.rule,
        (MemoryKind::Rule, "exceptions") => &mut fields.exceptions,
        (MemoryKind::Gotcha, "symptom") => &mut fields.symptom,
        (MemoryKind::Gotcha, "cause") => &mut fields.cause,
        (MemoryKind::Gotcha, "fix") => &mut fields.fix,
        (MemoryKind::Procedure, "step") => {
            fields.steps.push(value.to_owned());
            return true;
        }
        (MemoryKind::Procedure, "when") => &mut fields.when,
        (MemoryKind::Fact, "statement") => &mut fields.statement,
        (MemoryKind::Fact, "source") => &mut fields.source,
        _ => return false,
    };
    *target = Some(value.to_owned());
    true
}

#[cfg(test)]
mod tests;
