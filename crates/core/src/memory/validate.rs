use super::fields::Fields;
use super::model::{Kind, SaveInput};
use crate::error::Error;
use serde_json::Value;

pub const TITLE_MAX: usize = 80;
pub const SUMMARY_MAX: usize = 240;
pub const REASON_MAX: usize = 1_000;
pub const FIELD_TEXT_MAX: usize = 1_000;
pub const LIST_MAX_ITEMS: usize = 20;
pub const LIST_ITEM_MAX: usize = 300;
pub const FIELDS_MAX_BYTES: usize = 4 * 1024;
pub const ID_MAX: usize = 128;

fn bad<T>(message: impl Into<String>) -> Result<T, Error> {
    Err(Error::Validation(message.into()))
}

fn chars(text: &str) -> usize {
    text.chars().count()
}

/// Checks everything that needs no IO. Runs after redaction, so the stored text is what is checked.
/// `allow_handoff` is the system path (distill); agents never write a handoff.
pub fn validate_save(input: &SaveInput, fields: &Fields, allow_handoff: bool) -> Result<(), Error> {
    if input.kind == Kind::Handoff && !allow_handoff {
        return bad("kind handoff is written by the system only");
    }
    if input.title.trim().is_empty() || chars(&input.title) > TITLE_MAX {
        return bad(format!("title must be 1..={TITLE_MAX} characters"));
    }
    if input.summary.trim().is_empty() || chars(&input.summary) > SUMMARY_MAX {
        return bad(format!("summary must be 1..={SUMMARY_MAX} characters"));
    }
    if chars(&input.reason) > REASON_MAX {
        return bad(format!("reason must be at most {REASON_MAX} characters"));
    }
    if input.kind != Kind::Note && input.reason.trim().is_empty() {
        return bad(format!(
            "reason is required for kind {}",
            input.kind.as_str()
        ));
    }
    validate_list("evidence", &input.evidence)?;
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
            Value::String(text) if chars(text) > FIELD_TEXT_MAX => {
                return bad(format!(
                    "fields.{key} must be at most {FIELD_TEXT_MAX} characters"
                ));
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

fn validate_list(name: &str, items: &[String]) -> Result<(), Error> {
    if items.len() > LIST_MAX_ITEMS {
        return bad(format!("{name} must have at most {LIST_MAX_ITEMS} items"));
    }
    if items.iter().any(|item| chars(item) > LIST_ITEM_MAX) {
        return bad(format!(
            "each {name} item must be at most {LIST_ITEM_MAX} characters"
        ));
    }
    Ok(())
}

/// What an agent's save must carry to be worth keeping (CM-SPEC-008 §5). The first failing rule
/// wins; the message is one line the agent can act on. System saves are built to pass.
pub fn validate_quality(input: &SaveInput, fields: &Fields) -> Result<(), Error> {
    let quality = |message: &str| Err(Error::Quality(message.to_owned()));
    let (title, summary) = (input.title.trim(), input.summary.trim());
    if !(8..=TITLE_MAX).contains(&chars(title)) {
        return quality("title: use 8-80 characters that name the knowledge");
    }
    if !(20..=SUMMARY_MAX).contains(&chars(summary)) {
        return quality("summary: use 20-240 characters saying what is true");
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
mod tests {
    use super::*;
    use serde_json::json;

    fn check(value: serde_json::Value) -> Result<(), Error> {
        let input: SaveInput = serde_json::from_value(value).expect("shape");
        let fields = Fields::parse(input.kind, input.fields.clone()).expect("fields");
        validate_quality(&input, &fields)
    }

    fn base(kind: &str) -> serde_json::Value {
        json!({"kind": kind, "title": "Webhook handlers dedupe", "summary": "Handlers keep an event id table so replays do nothing.", "reason": "Providers retry deliveries."})
    }

    fn with(kind: &str, key: &str, value: serde_json::Value) -> serde_json::Value {
        let mut v = base(kind);
        v[key] = value;
        v
    }

    #[test]
    fn quality_messages() {
        let mut gotcha = base("gotcha");
        gotcha["fields"] = json!({"fix": "dedupe by event id"});
        let mut procedure = base("procedure");
        procedure["fields"] = json!({"steps": ["a", "b"]});
        for ok in [
            base("decision"),
            gotcha.clone(),
            procedure.clone(),
            with("note", "reason", json!("")),
        ] {
            assert_eq!(check(ok.clone()), Ok(()), "{ok}");
        }
        let cases = [
            (
                with("rule", "title", json!("short")),
                "title: use 8-80 characters that name the knowledge",
            ),
            (
                with("rule", "title", json!("x".repeat(81))),
                "title: use 8-80 characters that name the knowledge",
            ),
            (
                with("rule", "summary", json!("short summary")),
                "summary: use 20-240 characters saying what is true",
            ),
            (
                with("rule", "summary", json!("x".repeat(241))),
                "summary: use 20-240 characters saying what is true",
            ),
            (
                {
                    let mut v = base("rule");
                    v["summary"] = json!("WEBHOOK handlers dedupe!!  ");
                    v["title"] = json!("Webhook handlers dedupe");
                    v["summary"] = json!("webhook handlers dedupe.");
                    v
                },
                "summary: repeats the title; say what is true in 1-3 sentences",
            ),
            (
                with("rule", "reason", json!("because")),
                "reason: say why this is true or was chosen (10+ characters)",
            ),
            (base("gotcha"), "fields.fix: say how the gotcha was fixed"),
            (
                with("procedure", "fields", json!({"steps": ["only one"]})),
                "fields.steps: list at least 2 ordered steps",
            ),
        ];
        for (input, message) in cases {
            assert_eq!(
                check(input.clone()),
                Err(Error::Quality(message.to_owned())),
                "{input}"
            );
        }
        // the first failing rule wins
        let mut both = with("rule", "title", json!("short"));
        both["summary"] = json!("short");
        assert!(matches!(check(both), Err(Error::Quality(m)) if m.starts_with("title:")));
    }
}
