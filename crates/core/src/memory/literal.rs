//! Contiguous literal match for the console. NFC, case-insensitive, no stemming.
//!
//! PostgreSQL uses `lower(normalize(text, NFC))`. This folder uses the same steps in Rust so a
//! snippet lines up with the row the database returned.

use super::Fields;
use unicode_normalization::UnicodeNormalization;

pub const MAX_QUERY_CHARS: usize = 200;
const WINDOW: usize = 64;

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct MatchSegment {
    pub text: String,
    pub hit: bool,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct LiteralMatch {
    pub field: String,
    pub label: String,
    pub segments: Vec<MatchSegment>,
}

pub struct TextPart {
    pub field: &'static str,
    pub label: &'static str,
    pub text: String,
}

/// Trimmed query, or an error the API can show. Blank is browse mode, not an error.
pub fn check_query(raw: &str) -> Result<&str, &'static str> {
    let query = raw.trim();
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err("search is limited to 200 characters");
    }
    if query.contains('\0') {
        return Err("search cannot contain a null character");
    }
    Ok(query)
}

fn nfc(text: &str) -> String {
    text.nfc().collect()
}

/// NFC, then Unicode lowercase one character at a time so the map stays aligned.
fn fold_mapped(text: &str) -> (String, Vec<usize>) {
    let source = nfc(text);
    let mut folded = String::new();
    let mut map = Vec::new();
    for (index, ch) in source.chars().enumerate() {
        for _ in ch.to_lowercase() {
            map.push(index);
        }
        folded.extend(ch.to_lowercase());
    }
    (folded, map)
}

fn fold(text: &str) -> String {
    fold_mapped(text).0
}

pub fn contains_literal(haystack: &str, needle: &str) -> bool {
    let needle = fold(needle);
    !needle.is_empty() && fold(haystack).contains(&needle)
}

pub fn title_equals(title: &str, needle: &str) -> bool {
    let needle = fold(needle);
    !needle.is_empty() && fold(title) == needle
}

pub fn memory_parts(
    title: &str,
    summary: &str,
    reason: &str,
    detail: &str,
    fields: &Fields,
) -> Vec<TextPart> {
    let mut parts = Vec::new();
    push(&mut parts, "title", "Title", title);
    push(&mut parts, "summary", "Summary", summary);
    push(&mut parts, "reason", "Reason", reason);
    push(&mut parts, "detail", "Detail", detail);
    for (field, label, text) in fields.labeled() {
        parts.push(TextPart { field, label, text });
    }
    parts
}

fn push(parts: &mut Vec<TextPart>, field: &'static str, label: &'static str, text: &str) {
    if !text.is_empty() {
        parts.push(TextPart {
            field,
            label,
            text: text.to_owned(),
        });
    }
}

/// The first field that contains `needle`, with a short highlighted excerpt.
pub fn best_match(parts: &[TextPart], needle: &str) -> Option<LiteralMatch> {
    let part = parts
        .iter()
        .find(|part| contains_literal(&part.text, needle))?;
    Some(LiteralMatch {
        field: part.field.to_owned(),
        label: part.label.to_owned(),
        segments: highlight(&part.text, needle)?,
    })
}

fn highlight(text: &str, needle: &str) -> Option<Vec<MatchSegment>> {
    let source = nfc(text);
    let needle = fold(needle);
    if needle.is_empty() {
        return None;
    }
    let (folded, map) = fold_mapped(&source);
    let byte = folded.find(&needle)?;
    let start_folded = folded[..byte].chars().count();
    let end_folded = start_folded + needle.chars().count();
    if end_folded == 0 || end_folded > map.len() {
        return None;
    }
    let start = map[start_folded];
    let end = map[end_folded - 1] + 1;
    let chars: Vec<char> = source.chars().collect();
    if end > chars.len() {
        return None;
    }
    let from = start.saturating_sub(WINDOW);
    let to = (end + WINDOW).min(chars.len());
    let mut segments = Vec::new();
    piece(&mut segments, from > 0, "…", false);
    piece(
        &mut segments,
        from < start,
        &chars[from..start].iter().collect::<String>(),
        false,
    );
    piece(
        &mut segments,
        true,
        &chars[start..end].iter().collect::<String>(),
        true,
    );
    piece(
        &mut segments,
        end < to,
        &chars[end..to].iter().collect::<String>(),
        false,
    );
    piece(&mut segments, to < chars.len(), "…", false);
    Some(segments)
}

fn piece(segments: &mut Vec<MatchSegment>, keep: bool, text: &str, hit: bool) {
    if keep && !text.is_empty() {
        segments.push(MatchSegment {
            text: text.to_owned(),
            hit,
        });
    }
}

#[cfg(test)]
mod tests;
