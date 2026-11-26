//! Bounds for the ephemeral failed-command lookup.

use crate::capture::redact_sensitive_content;

pub const ERROR_TAIL_MAX_BYTES: usize = 2048;

pub fn error_tail(text: &str) -> String {
    let text = redact_sensitive_content(text);
    let start = text.ceil_char_boundary(text.len().saturating_sub(ERROR_TAIL_MAX_BYTES));
    text[start..].trim().to_owned()
}

pub fn first_error_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| {
        line.to_ascii_lowercase()
            .split(|c: char| !c.is_alphabetic())
            .any(|word| {
                matches!(word, "failed" | "panicked" | "traceback")
                    || word.ends_with("error")
                    || word.ends_with("exception")
            })
    })
}
