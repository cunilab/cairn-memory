use super::fields::Fields;
use super::model::{Kind, SaveInput};
use crate::error::Error;
use serde_json::Value;

/// Hard caps: a longer title or summary is refused. The suggested sizes below are what agent-facing cards
/// show and what system saves aim for; a save between the two is kept as sent, with a hint.
pub const TITLE_MAX: usize = 200;
pub const SUMMARY_MAX: usize = 1_000;
pub const TITLE_SUGGESTED: usize = 80;
pub const SUMMARY_SUGGESTED: usize = 240;
pub const REASON_MAX: usize = 1_000;
pub const DETAIL_MAX: usize = 8_000;
pub const FIELD_TEXT_MAX: usize = 1_000;
pub const LIST_MAX_ITEMS: usize = 20;
pub const LIST_ITEM_MAX: usize = 300;
pub const FIELDS_MAX_BYTES: usize = 4 * 1024;
pub const ID_MAX: usize = 128;
pub const ALIAS_MAX_ITEMS: usize = 5;
pub const ALIAS_MAX_CHARS: usize = 120;

fn bad<T>(message: impl Into<String>) -> Result<T, Error> {
    Err(Error::Validation(message.into()))
}

fn chars(text: &str) -> usize {
    text.chars().count()
}

fn validate_length(
    name: impl std::fmt::Display,
    text: &str,
    limit: usize,
    hint: &str,
) -> Result<(), Error> {
    let length = chars(text);
    if length > limit {
        return bad(format!(
            "{name} is {length} characters, limit {limit}: cut {}{hint}",
            length - limit
        ));
    }
    Ok(())
}

/// "summary is 264 characters; 240 suggested" for a title or summary over its suggested size.
pub fn length_hints(title: &str, summary: &str) -> Vec<String> {
    [
        ("title", title, TITLE_SUGGESTED),
        ("summary", summary, SUMMARY_SUGGESTED),
    ]
    .into_iter()
    .filter_map(|(name, text, suggested)| {
        let length = chars(text);
        (length > suggested)
            .then(|| format!("{name} is {length} characters; {suggested} suggested"))
    })
    .collect()
}

/// What an agent-facing card shows: at most `max` characters, cut on a character boundary with an
/// ellipsis. `memory_get` and the web show the full text.
pub fn card_text(text: &str, max: usize) -> std::borrow::Cow<'_, str> {
    match text.char_indices().nth(max.saturating_sub(1)) {
        Some((end, _)) if chars(text) > max => format!("{}\u{2026}", &text[..end]).into(),
        _ => text.into(),
    }
}

/// Checks everything that needs no IO. Runs after redaction, so the stored text is what is checked.
/// `allow_handoff` is the system path (distill); agents never write a handoff.
pub fn validate_save(input: &SaveInput, fields: &Fields, allow_handoff: bool) -> Result<(), Error> {
    if input.kind == Kind::Handoff && !allow_handoff {
        return bad("kind handoff is written by the system only");
    }
    if input.title.trim().is_empty() {
        return bad("title must not be empty");
    }
    validate_length("title", &input.title, TITLE_MAX, "")?;
    if input.summary.trim().is_empty() {
        return bad("summary must not be empty");
    }
    validate_length(
        "summary",
        &input.summary,
        SUMMARY_MAX,
        ", or move extra context into reason or detail",
    )?;
    validate_length(
        "reason",
        &input.reason,
        REASON_MAX,
        ", or move background into detail",
    )?;
    validate_length(
        "detail",
        input.detail.as_deref().unwrap_or_default(),
        DETAIL_MAX,
        "",
    )?;
    if input.kind != Kind::Note && input.reason.trim().is_empty() {
        return bad(format!(
            "reason must not be empty for kind {}",
            input.kind.as_str()
        ));
    }
    validate_list("evidence", &input.evidence)?;
    // Agent rows use ords 0..4. A `learned` row (item 15) shares this cap of 5.
    validate_aliases(&input.aliases)?;
    for (name, id) in [
        ("supersedes", &input.supersedes),
        ("contradicts", &input.contradicts),
    ] {
        if let Some(id) = id
            && (id.trim().is_empty() || id.len() > ID_MAX)
        {
            return bad(format!("{name} must be a memory id"));
        }
    }
    validate_fields(fields)
}

/// Each string at most 1,000 chars, each array at most 20 items of at most 300 chars, the whole
/// object at most 4 KiB.
pub fn validate_fields(fields: &Fields) -> Result<(), Error> {
    let value = fields.to_json();
    let Value::Object(map) = &value else {
        return bad("fields must be an object");
    };
    for (key, item) in map {
        match item {
            Value::String(text) => {
                validate_length(format_args!("fields.{key}"), text, FIELD_TEXT_MAX, "")?;
            }
            Value::Array(items) => {
                let texts: Vec<String> = items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect();
                validate_list(&format!("fields.{key}"), &texts)?;
            }
            _ => {}
        }
    }
    if value.to_string().len() > FIELDS_MAX_BYTES {
        return bad(format!("fields must be at most {FIELDS_MAX_BYTES} bytes"));
    }
    Ok(())
}

fn validate_aliases(aliases: &[String]) -> Result<(), Error> {
    if aliases.len() > ALIAS_MAX_ITEMS {
        return bad(format!("aliases must have at most {ALIAS_MAX_ITEMS} items"));
    }
    if aliases
        .iter()
        .any(|alias| alias.trim().is_empty() || chars(alias) > ALIAS_MAX_CHARS)
    {
        return bad(format!(
            "each alias must be 1..={ALIAS_MAX_CHARS} characters"
        ));
    }
    Ok(())
}

fn validate_list(name: &str, items: &[String]) -> Result<(), Error> {
    if items.len() > LIST_MAX_ITEMS {
        return bad(format!("{name} must have at most {LIST_MAX_ITEMS} items"));
    }
    for (index, item) in items.iter().enumerate() {
        validate_length(format_args!("{name}[{index}]"), item, LIST_ITEM_MAX, "")?;
    }
    Ok(())
}

/// What an agent's save must carry to be worth keeping (apps/docs/content/users/reference/mcp.md). The first failing rule
/// wins; the message is one line the agent can act on. System saves are built to pass.
pub fn validate_quality(input: &SaveInput, fields: &Fields) -> Result<(), Error> {
    let quality = |message: &str| Err(Error::Quality(message.to_owned()));
    let (title, summary) = (input.title.trim(), input.summary.trim());
    if !(8..=TITLE_MAX).contains(&chars(title)) {
        return quality("title: use 8-200 characters (about 80) that name the knowledge");
    }
    if !(20..=SUMMARY_MAX).contains(&chars(summary)) {
        return quality("summary: use 20-1000 characters (about 240) saying what is true");
    }
    let bare = |text: &str| {
        text.trim_end_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
            .to_lowercase()
    };
    if bare(summary) == bare(title) {
        return quality("summary: repeats the title; say what is true in 1-3 sentences");
    }
    if input.kind != Kind::Note && chars(input.reason.trim()) < 10 {
        return quality("reason: say why this is true or was chosen (10+ characters)");
    }
    match fields {
        Fields::Gotcha(gotcha) if gotcha.fix.trim().is_empty() => {
            quality("fields.fix: say how the gotcha was fixed")
        }
        Fields::Procedure(procedure)
            if procedure
                .steps
                .iter()
                .filter(|s| !s.trim().is_empty())
                .count()
                < 2 =>
        {
            quality("fields.steps: list at least 2 ordered steps")
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;
