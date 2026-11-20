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
