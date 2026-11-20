use crate::legacy::memories::Memory;
use crate::legacy::types::WorkContext;

pub(crate) const REDACTED: &str = "[REDACTED]";

pub(crate) fn human_context_texts(context: &WorkContext) -> impl Iterator<Item = &str> {
    context
        .files
        .iter()
        .chain(context.symbols.iter())
        .chain(context.commands.iter())
        .chain(context.tests.iter())
        .chain(context.services.iter())
        .chain(context.handoffs.iter())
        .chain(context.runbooks.iter())
        .map(String::as_str)
}

fn redact_string_list(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| redact_sensitive_content(value))
        .collect()
}

/// Redacts the human-readable work-context lists. Repository, branch,
/// worktree, and commit stay, including a supported secret in those fields.
pub fn redact_work_context(context: &WorkContext) -> WorkContext {
    WorkContext {
        project_id: context.project_id.clone(),
        repository: context.repository.clone(),
        branch: context.branch.clone(),
        worktree: context.worktree.clone(),
        commit: context.commit.clone(),
        files: redact_string_list(&context.files),
        symbols: redact_string_list(&context.symbols),
        commands: redact_string_list(&context.commands),
        tests: redact_string_list(&context.tests),
        services: redact_string_list(&context.services),
        handoffs: redact_string_list(&context.handoffs),
        runbooks: redact_string_list(&context.runbooks),
    }
}

/// Redacts human-readable memory metadata on a new row. Ids, timestamps,
/// commits, and provenance links stay.
pub fn redact_memory_metadata(memory: &mut Memory) {
    let fields = &mut memory.fields;
    for text in [&mut fields.title, &mut fields.summary, &mut fields.reason] {
        *text = redact_sensitive_content(text);
    }
    for text in [
        &mut fields.details,
        &mut fields.decision_context,
        &mut fields.consequences,
        &mut fields.rule,
        &mut fields.exceptions,
        &mut fields.symptom,
        &mut fields.cause,
        &mut fields.fix,
        &mut fields.when,
        &mut fields.statement,
        &mut fields.source,
        &mut fields.preference,
        &mut fields.example,
    ]
    .into_iter()
    .flatten()
    {
        *text = redact_sensitive_content(text);
    }
    for texts in [
        &mut fields.applies_to,
        &mut fields.evidence,
        &mut fields.tags,
        &mut fields.entities,
        &mut fields.alternatives,
        &mut fields.steps,
    ] {
        *texts = redact_string_list(texts);
    }

    memory.context = redact_work_context(&memory.context);
    memory.next_steps = redact_string_list(&memory.next_steps);
    memory.unresolved_work = redact_string_list(&memory.unresolved_work);
    memory.promotion_reason = memory
        .promotion_reason
        .as_deref()
        .map(redact_sensitive_content);
}

pub fn redact_sensitive_content(content: &str) -> String {
    let mut redacted = content.to_owned();
    for marker in ["authorization: bearer ", "api_key=", "token=", "password="] {
        redacted = redact_after_marker(&redacted, marker);
    }
    redacted = redact_prefixed_secret(&redacted, "sk-");
    redact_private_key_blocks(&redacted)
}

fn redact_after_marker(content: &str, marker: &str) -> String {
    let lower = content.to_ascii_lowercase();
    let mut output = String::with_capacity(content.len());
    let mut cursor = 0;

    while let Some(relative) = lower[cursor..].find(marker) {
        let start = cursor + relative;
        let value_start = start + marker.len();
        output.push_str(&content[cursor..value_start]);
        let value_end = content[value_start..]
            .find(char::is_whitespace)
            .map(|offset| value_start + offset)
            .unwrap_or(content.len());
        output.push_str(REDACTED);
        cursor = value_end;
    }

    output.push_str(&content[cursor..]);
    output
}

// A prefix counts only where a token starts, so ordinary words such as "disk-full" stay intact.
pub(crate) fn find_token_prefix(lower: &str, prefix: &str, from: usize) -> Option<usize> {
    let mut search_from = from;
    while let Some(relative) = lower[search_from..].find(prefix) {
        let start = search_from + relative;
        if !lower[..start]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric)
        {
            return Some(start);
        }
        search_from = start + prefix.len();
    }
    None
}

fn redact_prefixed_secret(content: &str, prefix: &str) -> String {
    let lower = content.to_ascii_lowercase();
    let prefix = prefix.to_ascii_lowercase();
    let mut output = String::with_capacity(content.len());
    let mut cursor = 0;
    while let Some(start) = find_token_prefix(&lower, &prefix, cursor) {
        let value_end = content[start..]
            .find(char::is_whitespace)
            .map(|offset| start + offset)
            .unwrap_or(content.len());
        output.push_str(&content[cursor..start]);
        output.push_str(REDACTED);
        cursor = value_end;
    }
    output.push_str(&content[cursor..]);
    output
}

const PRIVATE_KEY_LABELS: &[&str] = &[
    "encrypted private key",
    "openssh private key",
    "rsa private key",
    "ec private key",
    "private key",
];

fn known_private_key_label(label: &str) -> Option<&'static str> {
    PRIVATE_KEY_LABELS
        .iter()
        .copied()
        .find(|candidate| *candidate == label)
}

fn private_key_header(line: &str) -> Option<(usize, &'static str)> {
    let lower = line.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(relative) = lower[search_from..].find("-----begin ") {
        let start = search_from + relative;
        let after = start + "-----begin ".len();
        let Some(end_rel) = lower[after..].find("-----") else {
            break;
        };
        let label_end = after + end_rel;
        if let Some(label) = known_private_key_label(&lower[after..label_end]) {
            return Some((start, label));
        }
        search_from = label_end + "-----".len();
    }
    None
}

pub(crate) fn contains_private_key_header(content: &str) -> bool {
    let mut cursor = 0;
    while cursor < content.len() {
        let next_newline = content[cursor..]
            .find('\n')
            .map(|offset| cursor + offset + 1)
            .unwrap_or(content.len());
        if private_key_header(&content[cursor..next_newline]).is_some() {
            return true;
        }
        cursor = next_newline;
    }
    false
}

fn is_pem_continuation_line(line: &str) -> bool {
    let trimmed = line.trim_end_matches(['\r', '\n']).trim();
    if trimmed.is_empty() {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("proc-type:") || lower.starts_with("dek-info:") {
        return true;
    }
    trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '='))
}

/// Resume offset after optional PEM base64 glued to a truncated
/// `-----END <label>` that has no closing `-----`.
fn truncated_footer_resume(line_lower: &str, label: &str) -> Option<usize> {
    let token = format!("-----end {label}");
    let bytes = line_lower.as_bytes();
    let mut index = 0;
    while index < bytes.len()
        && matches!(bytes[index], b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' | b'=')
    {
        index += 1;
    }
    let rest = line_lower.get(index..)?;
    if !rest.starts_with(&token) {
        return None;
    }
    let after = index + token.len();
    if line_lower
        .get(after..)
        .is_some_and(|suffix| suffix.starts_with("-----"))
    {
        return None;
    }
    Some(after)
}

/// A private-key header with no matching END drops that header line and
/// following base64 or OpenSSL metadata lines. Base64 glued to a truncated
/// `-----END <label>` drops that prefix and the footer token; text after the
/// token stays.
fn redact_private_key_blocks(content: &str) -> String {
    let mut output = String::with_capacity(content.len());
    let mut cursor = 0;
    while cursor < content.len() {
        let next_newline = content[cursor..]
            .find('\n')
            .map(|offset| cursor + offset + 1)
            .unwrap_or(content.len());
        let line = &content[cursor..next_newline];
        let Some((header_at, label)) = private_key_header(line) else {
            output.push_str(line);
            cursor = next_newline;
            continue;
        };
        output.push_str(&line[..header_at]);
        output.push_str(REDACTED);
        let end_marker = format!("-----end {label}-----");
        let line_lower = line.to_ascii_lowercase();
        if let Some(end_rel) = line_lower[header_at..].find(&end_marker) {
            cursor += header_at + end_rel + end_marker.len();
            continue;
        }
        if line.ends_with('\n') {
            output.push('\n');
        }
        cursor = next_newline;
        while cursor < content.len() {
            let body_end = content[cursor..]
                .find('\n')
                .map(|offset| cursor + offset + 1)
                .unwrap_or(content.len());
            let body = &content[cursor..body_end];
            let body_lower = body.to_ascii_lowercase();
            if let Some(end_rel) = body_lower.find(&end_marker) {
                cursor += end_rel + end_marker.len();
                break;
            }
            if let Some(resume) = truncated_footer_resume(&body_lower, label) {
                cursor += resume;
                break;
            }
            if !is_pem_continuation_line(body) {
                break;
            }
            cursor = body_end;
        }
    }
    output
}

#[cfg(test)]
mod tests;
