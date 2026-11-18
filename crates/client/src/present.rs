//! Agent-facing MCP results. REST and the CLI keep the server JSON; only `tools/call`
//! uses this module.
//!
//! TOON text is limited to the graph and similarity-review tools (TOON spec 4.4, tab
//! delimiter). It is not a general encoder: a shape it does not support falls back to
//! compact JSON.

use serde_json::{Map, Value, json};

const CARD_KEYS: &[&str] = &["memory_id", "kind", "scope", "title", "summary", "score"];
/// A label, the same width as a capture's 120-character first line for ASCII.
const CARD_TITLE_MAX_BYTES: usize = 120;
/// About two short sentences, far below a capture body.
const CARD_SUMMARY_MAX_BYTES: usize = 240;
const CARD_ELLIPSIS: &str = "…";

/// Useful recall fields. Lifecycle, internal ids, and the duplicated `content` body are absent.
const EXPANDED_KEYS: &[&str] = &[
    "memory_id",
    "kind",
    "scope",
    "project_id",
    "title",
    "summary",
    "score",
    "status",
    "pinned",
    "importance",
    "details",
    "reason",
    "applies_to",
    "evidence",
    "tags",
    "entities",
    "decision_context",
    "alternatives",
    "consequences",
    "rule",
    "exceptions",
    "symptom",
    "cause",
    "fix",
    "steps",
    "when",
    "statement",
    "preference",
    "example",
    "next_steps",
    "unresolved_work",
    "decision_reason",
    "relationships",
    "stale",
    "stale_reason",
    "contradictory",
    "contradiction_ids",
    "supersedes",
    "promotion_reason",
    "verified_commit",
];

const PACK_KEYS: &[&str] = &[
    "query",
    "results",
    "omitted_count",
    "omitted_memory_ids",
    "recovery_hint",
    "mode",
    "note",
];

pub fn present(name: &str, expanded: bool, value: Value) -> Value {
    match name {
        "memory_save" | "memory_lesson_save" => same(receipt(&value)),
        "memory_get" | "memory_export" => same(value),
        "memory_search"
        | "memory_smart_search"
        | "memory_recall"
        | "memory_context"
        | "memory_reflect" => same(shape_hits(value, expanded)),
        "memory_why_changed" | "memory_resume" | "memory_unresolved" => {
            same(shape_hits_with(value, |row| omit_empty(row.clone())))
        }
        "memory_graph" => toon_channels(value),
        "memory_similarity_reviews" => toon_channels(wrap("reviews", value)),
        "memory_list" | "memory_project_memories" => same(omit_empty(wrap("memories", value))),
        "memory_project_list" => same(omit_empty(wrap("projects", value))),
        "memory_project_members" => same(omit_empty(wrap("members", value))),
        "memory_project_freshness" => same(omit_empty(wrap("freshness", value))),
        _ => same(omit_empty(value)),
    }
}

fn same(value: Value) -> Value {
    success(value.clone(), compact(&value))
}

fn toon_channels(value: Value) -> Value {
    let text = toon_document(&value).unwrap_or_else(|| compact(&value));
    success(omit_empty(value), text)
}

fn success(structured: Value, text: String) -> Value {
    json!({
        "isError": false,
        "content": [{"type": "text", "text": text}],
        "structuredContent": structured
    })
}

fn compact(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

fn receipt(value: &Value) -> Value {
    json!({
        "memory_id": field(value, "memory_id"),
        "kind": field(value, "kind"),
        "scope": field(value, "scope"),
        "status": field(value, "status"),
    })
}

fn field(value: &Value, key: &str) -> Value {
    value.get(key).cloned().unwrap_or(Value::Null)
}

fn wrap(name: &str, value: Value) -> Value {
    match value {
        Value::Array(items) => json!({ name: items }),
        other => other,
    }
}

fn shape_hits(value: Value, expanded: bool) -> Value {
    shape_hits_with(value, |row| project_row(row, expanded))
}

fn shape_hits_with(value: Value, project: impl Fn(&Value) -> Value) -> Value {
    match value {
        Value::Array(rows) => json!({ "results": rows.iter().map(&project).collect::<Vec<_>>() }),
        Value::Object(map) => Value::Object(shape_object(map, project)),
        other => other,
    }
}

fn shape_object(map: Map<String, Value>, project: impl Fn(&Value) -> Value) -> Map<String, Value> {
    let mut out = Map::new();
    for (key, child) in map {
        if key == "results" {
            let rows = child
                .as_array()
                .map(|rows| rows.iter().map(&project).collect())
                .unwrap_or_default();
            out.insert(key, Value::Array(rows));
            continue;
        }
        if PACK_KEYS.contains(&key.as_str()) {
            out.insert(key, child);
            continue;
        }
        if !empty_optional(&child) {
            out.insert(key, omit_empty(child));
        }
    }
    out
}

fn project_row(row: &Value, expanded: bool) -> Value {
    let keys: &[&str] = if expanded { EXPANDED_KEYS } else { CARD_KEYS };
    let mut out = Map::new();
    for key in keys {
        if CARD_KEYS.contains(key) {
            let value = if expanded || !matches!(*key, "title" | "summary") {
                field(row, key)
            } else {
                clip_card_text(row, key)
            };
            out.insert((*key).to_owned(), value);
            continue;
        }
        if let Some(value) = row.get(*key)
            && !empty_optional(value)
            && !value.as_array().is_some_and(Vec::is_empty)
            && !value.as_object().is_some_and(Map::is_empty)
        {
            out.insert((*key).to_owned(), value.clone());
        }
    }
    Value::Object(out)
}

fn empty_optional(value: &Value) -> bool {
    value.is_null() || value.as_str().is_some_and(str::is_empty)
}

/// Same character-boundary cut as the session brief, with the ellipsis inside the budget.
fn clip_card_text(row: &Value, key: &str) -> Value {
    let Some(text) = row.get(key).and_then(Value::as_str) else {
        return field(row, key);
    };
    let max = if key == "title" {
        CARD_TITLE_MAX_BYTES
    } else {
        CARD_SUMMARY_MAX_BYTES
    };
    if text.len() <= max {
        return Value::String(text.to_owned());
    }
    let mut end = max.saturating_sub(CARD_ELLIPSIS.len()).min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut clipped = String::with_capacity(end + CARD_ELLIPSIS.len());
    clipped.push_str(&text[..end]);
    clipped.push_str(CARD_ELLIPSIS);
    Value::String(clipped)
}

fn omit_empty(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, child) in map {
                if empty_optional(&child) {
                    continue;
                }
                out.insert(key, omit_empty(child));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(omit_empty).collect()),
        other => other,
    }
}

/// Tab-delimited TOON for one flat object of primitives and uniform primitive-object arrays.
/// Spec 4.4: no trailing newline, nulls kept so a table stays rectangular.
fn toon_document(value: &Value) -> Option<String> {
    let Value::Object(map) = value else {
        return None;
    };
    let mut lines = Vec::new();
    for (key, child) in map {
        lines.extend(encode_field(key, child)?);
    }
    Some(lines.join("\n"))
}

fn encode_field(key: &str, value: &Value) -> Option<Vec<String>> {
    match value {
        Value::Array(items) if items.is_empty() => Some(vec![format!("{key}: []")]),
        Value::Array(items) => encode_table(key, items),
        Value::Object(_) => None,
        primitive => Some(vec![format!(
            "{}: {}",
            encode_key(key),
            encode_primitive(primitive)?
        )]),
    }
}

fn encode_table(key: &str, items: &[Value]) -> Option<Vec<String>> {
    let mut fields: Vec<&str> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let Value::Object(map) = item else {
            return None;
        };
        if map.is_empty()
            || map
                .values()
                .any(|value| value.is_array() || value.is_object())
        {
            return None;
        }
        let keys: Vec<&str> = map.keys().map(String::as_str).collect();
        if index == 0 {
            fields = keys;
        } else if keys.len() != fields.len() || keys.iter().zip(&fields).any(|(a, b)| a != b) {
            return None;
        }
    }
    let header = format!(
        "{key}[{}\t]{{{}}}:",
        items.len(),
        fields
            .iter()
            .map(|field| encode_key(field))
            .collect::<Vec<_>>()
            .join("\t")
    );
    let mut lines = vec![header];
    for item in items {
        let map = item.as_object()?;
        let cells = fields
            .iter()
            .map(|field| encode_primitive(map.get(*field)?))
            .collect::<Option<Vec<_>>>()?;
        lines.push(format!("  {}", cells.join("\t")));
    }
    Some(lines)
}

fn encode_key(key: &str) -> String {
    let plain = !key.is_empty()
        && key
            .chars()
            .enumerate()
            .all(|(index, c)| c.is_ascii_alphanumeric() || c == '_' || (index > 0 && c == '.'))
        && key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if plain {
        key.to_owned()
    } else {
        format!("\"{}\"", escape(key))
    }
}

fn encode_primitive(value: &Value) -> Option<String> {
    Some(match value {
        Value::Null => "null".to_owned(),
        Value::Bool(true) => "true".to_owned(),
        Value::Bool(false) => "false".to_owned(),
        Value::Number(number) => encode_number(number),
        Value::String(text) => encode_string(text),
        Value::Array(_) | Value::Object(_) => return None,
    })
}

fn encode_number(number: &serde_json::Number) -> String {
    if let Some(value) = number.as_i64() {
        return value.to_string();
    }
    if let Some(value) = number.as_u64() {
        return value.to_string();
    }
    let Some(value) = number.as_f64() else {
        return number.to_string();
    };
    if !value.is_finite() {
        return "null".to_owned();
    }
    serde_json::to_string(&value).unwrap_or_else(|_| number.to_string())
}

fn encode_string(text: &str) -> String {
    if needs_quotes(text) {
        format!("\"{}\"", escape(text))
    } else {
        text.to_owned()
    }
}

fn needs_quotes(text: &str) -> bool {
    if text.is_empty()
        || text.starts_with([' ', '\t', '-', '#'])
        || text.ends_with([' ', '\t'])
        || matches!(text, "true" | "false" | "null")
        || numeric_like(text)
    {
        return true;
    }
    text.chars()
        .any(|c| matches!(c, ':' | '"' | '\\' | '[' | ']' | '{' | '}' | '\t') || c.is_control())
}

fn numeric_like(text: &str) -> bool {
    let rest = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (digits, rest) = take_digits(rest);
    if digits == 0 {
        return false;
    }
    let rest = if let Some(fraction) = rest.strip_prefix('.') {
        let (fraction_digits, rest) = take_digits(fraction);
        if fraction_digits == 0 {
            return false;
        }
        rest
    } else {
        rest
    };
    let Some(exponent) = rest.strip_prefix(['e', 'E']) else {
        return rest.is_empty();
    };
    let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
    let (exponent_digits, rest) = take_digits(exponent);
    exponent_digits > 0 && rest.is_empty()
}

fn take_digits(text: &str) -> (usize, &str) {
    let digits = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    (digits, &text[digits..])
}

fn escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let code = c as u32;
                out.push_str(&format!("\\u{code:04x}"));
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests;
