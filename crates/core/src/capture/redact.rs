//! Redaction of secrets before anything is hashed or stored. Ported from the 0.1.5 denylist;
//! widening it is a later step.

use super::observation::WorkContext;

pub const REDACTED: &str = "[REDACTED]";

fn redact_list(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| redact_sensitive_content(value))
        .collect()
}

/// Redacts the human-readable lists. Project, repository, branch, worktree and commit stay.
pub fn redact_work_context(context: &WorkContext) -> WorkContext {
    WorkContext {
        files: redact_list(&context.files),
        symbols: redact_list(&context.symbols),
        commands: redact_list(&context.commands),
        tests: redact_list(&context.tests),
        services: redact_list(&context.services),
        ..context.clone()
    }
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
fn find_token_prefix(lower: &str, prefix: &str, from: usize) -> Option<usize> {
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

pub fn contains_private_key_header(content: &str) -> bool {
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
mod tests {
    use super::*;

    #[test]
    fn redacts_markers_and_sk_tokens_but_not_ordinary_words() {
        let text = "password=hunter2 and token=abc, key sk-AbC123 disk-full ok";
        let out = redact_sensitive_content(text);
        assert!(!out.contains("hunter2") && !out.contains("abc,") && !out.contains("AbC123"));
        assert!(out.contains("disk-full ok"));
        assert!(redact_sensitive_content("Authorization: Bearer xyz").ends_with(REDACTED));
    }

    #[test]
    fn redacts_private_key_blocks() {
        let text = "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIabc+/=\n-----END RSA PRIVATE KEY-----\nafter";
        let out = redact_sensitive_content(text);
        assert!(out.contains("before") && out.contains("after"));
        assert!(!out.contains("MIIabc") && out.contains(REDACTED));
        assert!(contains_private_key_header(text));
    }

    #[test]
    fn context_lists_are_redacted_but_git_state_is_not() {
        let context = WorkContext {
            branch: Some("main".into()),
            commands: vec!["curl -H token=secret1".into()],
            ..WorkContext::default()
        };
        let out = redact_work_context(&context);
        assert_eq!(out.branch.as_deref(), Some("main"));
        assert!(!out.commands[0].contains("secret1"));
    }
}
