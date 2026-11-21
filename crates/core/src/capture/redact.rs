//! Redaction of secrets before anything is hashed or stored. Ported from the 0.1.5 denylist;
//! widening it is a later step.

use super::observation::WorkContext;
use regex::Regex;
use std::borrow::Cow;
use std::sync::OnceLock;

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

/// Rows 2-11 of CM-SPEC-006 §3, in application order; row 1 (PEM blocks) runs first.
const PATTERNS: &[(&str, &str)] = &[
    (
        r#"(?i)\b(authorization\s*:\s*(?:bearer|basic|token|digest))\s+[^\s"']+"#,
        "${1} [REDACTED]",
    ),
    (
        r"(?i)\b([a-z][a-z0-9+.-]*://)[^\s/:@]+:[^\s/@]+@",
        "${1}[REDACTED]@",
    ),
    (
        r#"(?i)(--(?:password|passwd|pass|token|secret|api-key|apikey|access-key|client-secret))(=|\s+)("[^"]*"|'[^']*'|\S+)"#,
        "${1}${2}[REDACTED]",
    ),
    (
        r#"(?i)\b((?:api[_-]?key|access[_-]?key(?:[_-]?id)?|secret(?:[_-]?key)?|client[_-]?secret|token|auth[_-]?token|password|passwd|pwd))(\s*[=:]\s*)("[^"]*"|'[^']*'|[^\s,;&]+)"#,
        "${1}${2}[REDACTED]",
    ),
    (r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36,}\b", REDACTED),
    (r"\bgithub_pat_[A-Za-z0-9_]{22,}\b", REDACTED),
    (r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b", REDACTED),
    (r"\bxox[abprs]-[A-Za-z0-9-]{10,}\b", REDACTED),
    (r"\bsk-(?:ant-|proj-)?[A-Za-z0-9_-]{20,}\b", REDACTED),
    (
        r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b",
        REDACTED,
    ),
    (r"\bcdt_[A-Za-z0-9_-]{16,}\b", REDACTED),
];

fn compiled() -> &'static [(Regex, &'static str)] {
    static COMPILED: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    COMPILED.get_or_init(|| {
        PATTERNS
            .iter()
            .map(|(pattern, with)| (Regex::new(pattern).expect("redaction pattern"), *with))
            .collect()
    })
}

/// Idempotent: `redact(redact(x)) == redact(x)`.
pub fn redact_sensitive_content(content: &str) -> String {
    let mut redacted = redact_private_key_blocks(content);
    for (pattern, with) in compiled() {
        if let Cow::Owned(next) = pattern.replace_all(&redacted, *with) {
            redacted = next;
        }
    }
    redacted
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
    fn markers_and_patterns() {
        // One case per row of the spec's table (CM-SPEC-006 §3): (input, text that must go).
        let cases = [
            ("Authorization: Bearer abc.def-123", "abc.def-123"),
            ("authorization: basic dXNlcjpwYXNz", "dXNlcjpwYXNz"),
            ("postgres://admin:hunter2@db:5432/x", "hunter2"),
            ("run --password hunter2 --token=abc123 now", "hunter2"),
            ("run --client-secret \"two words\" now", "two words"),
            ("api_key=zzz111 and password: 'a b c'", "zzz111"),
            ("SECRET_KEY = s3cr3t,", "s3cr3t"),
            ("ghp_0123456789abcdefghijklmnopqrstuvwxyzAB", "ghp_0123"),
            ("github_pat_0123456789abcdefghijkl_xyz", "github_pat_"),
            ("id AKIAIOSFODNN7EXAMPLE end", "AKIAIOSFODNN7EXAMPLE"),
            ("slack xoxb-1234567890-abcdef", "xoxb-"),
            ("key sk-ant-0123456789abcdefghijklmn", "sk-ant-"),
            (
                "jwt eyJhbGciOiJIUzI1.eyJzdWIiOiIxMjM0.SflKxwRJSMeKKF2QT4f",
                "eyJhbGci",
            ),
            ("cdt_0123456789abcdefghij", "cdt_0123"),
        ];
        for (input, secret) in cases {
            let out = redact_sensitive_content(input);
            assert!(!out.contains(secret), "{input} -> {out}");
            assert!(out.contains(REDACTED), "{input} -> {out}");
        }
        let out = redact_sensitive_content("token=abc, disk-full ok sk-short");
        assert_eq!(out, "token=[REDACTED], disk-full ok sk-short");
        assert!(redact_sensitive_content("Authorization: Bearer xyz").ends_with(REDACTED));
    }

    #[test]
    fn idempotent() {
        let text = "Authorization: Bearer a1b2\npostgres://u:p@h/db --password x token=y sk-0123456789abcdefghij\n-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----";
        let once = redact_sensitive_content(text);
        assert_eq!(redact_sensitive_content(&once), once);
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
