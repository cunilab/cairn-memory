use super::{Fields, Kind};
use crate::capture::redact_sensitive_content;

const MAX_HINTS: usize = 4;
const MAX_HINT_BYTES: usize = 120;
const GENERIC_WORDS: &[&str] = &[
    "fix", "update", "bug", "change", "note", "misc", "stuff", "todo", "wip", "test", "task",
    "issue", "work", "patch", "other",
];

/// Advise on a validated save. The duplicate, if any, has already been checked for visibility.
pub fn save_hints(
    title: &str,
    summary: &str,
    reason: &str,
    fields: &Fields,
    duplicate: Option<(&str, &str)>,
) -> Vec<String> {
    if fields.kind() == Kind::Handoff {
        return Vec::new();
    }
    let title = words(title);
    let summary = words(summary);
    let reason = reason.trim();
    let reason_words = words(reason);
    let mut hints = Vec::new();
    if let Some((id, title)) = duplicate {
        let prefix = format!("Looks like {id} '");
        let suffix = format!("'. Use supersedes={id}.");
        if let Some(budget) = MAX_HINT_BYTES.checked_sub(prefix.len() + suffix.len()) {
            let title = redact_sensitive_content(title);
            let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
            let title = &title[..title.floor_char_boundary(budget)];
            hints.push(format!("{prefix}{title}{suffix}"));
        }
    }
    if (!reason.is_empty() || fields.kind() != Kind::Note)
        && (reason.chars().count() < 20
            || reason_words.is_empty()
            || reason_words == title
            || reason_words == summary)
    {
        hints.push(
            "Reason should explain why this is true or was chosen in at least 20 characters."
                .into(),
        );
    }
    if title.len() < 3
        || title
            .iter()
            .all(|word| GENERIC_WORDS.contains(&word.as_str()))
    {
        hints.push("Use at least three specific words in the title to name the lesson.".into());
    }
    if summary.len() < 5 || title.windows(summary.len()).any(|part| part == summary) {
        hints.push("Write a summary of at least five words that adds to the title.".into());
    }
    let fields = match fields {
        Fields::Gotcha(fields)
            if [&fields.symptom, &fields.cause, &fields.fix]
                .iter()
                .any(|text| text.trim().is_empty()) =>
        {
            Some("Fill in the gotcha's symptom, cause and fix.")
        }
        Fields::Decision(fields)
            if fields.context.trim().is_empty()
                && !fields
                    .alternatives
                    .iter()
                    .any(|text| !text.trim().is_empty()) =>
        {
            Some("Add decision context or list the alternatives you considered.")
        }
        Fields::Procedure(fields) if !fields.steps.iter().any(|text| !text.trim().is_empty()) => {
            Some("List the steps of the procedure in order.")
        }
        _ => None,
    };
    hints.extend(fields.map(str::to_owned));
    hints.truncate(MAX_HINTS);
    hints
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests;
