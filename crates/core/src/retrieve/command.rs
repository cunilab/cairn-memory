//! Bounds for the ephemeral failed-command lookup.

use crate::capture::redact_sensitive_content;

pub const ERROR_TAIL_MAX_BYTES: usize = 2048;

pub fn error_tail(text: &str) -> String {
    let text = redact_sensitive_content(text);
    let start = text.ceil_char_boundary(text.len().saturating_sub(ERROR_TAIL_MAX_BYTES));
    text[start..].trim().to_owned()
}

pub fn first_error_line(text: &str) -> Option<&str> {
    let mut lines = text.lines();
    let line = lines.by_ref().map(str::trim).find(|line| {
        line.to_ascii_lowercase()
            .split(|c: char| !c.is_alphabetic())
            .any(|word| {
                matches!(word, "failed" | "panicked" | "traceback")
                    || word.ends_with("error")
                    || word.ends_with("exception")
            })
    })?;
    if !line.starts_with("Traceback (most recent call last)") {
        return Some(line);
    }
    // Python indents the frames and prints the exception (`KeyError: 'x'`) unindented after them.
    lines
        .find(|line| !line.trim().is_empty() && !line.starts_with(char::is_whitespace))
        .map(str::trim)
        .or(Some(line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_python_traceback_gives_its_exception_line() {
        let stderr = "Traceback (most recent call last):\n  File \"/w/app.py\", line 9, in <module>\n    main()\n  File \"/w/app.py\", line 5, in main\n    import violet_latch\n    ^^^^^^^^^^^^^^^^^^^\nModuleNotFoundError: No module named 'violet_latch'\n";
        assert_eq!(
            first_error_line(stderr),
            Some("ModuleNotFoundError: No module named 'violet_latch'")
        );
        let chained = "Traceback (most recent call last):\n  File \"x.py\", line 1, in <module>\nKeyError: 'x'\n\nDuring handling of the above exception, another exception occurred:\n";
        assert_eq!(first_error_line(chained), Some("KeyError: 'x'"));
        // A header with nothing after it is still the best line there is.
        assert_eq!(
            first_error_line("Traceback (most recent call last):\n  File \"x.py\""),
            Some("Traceback (most recent call last):")
        );
    }

    #[test]
    fn other_errors_pick_the_same_line_as_before() {
        let cargo = "   Compiling cairn v0.1.12\nerror[E0308]: mismatched types\n  --> src/a.rs:3:5\nerror: could not compile `cairn`";
        assert_eq!(
            first_error_line(cargo),
            Some("error[E0308]: mismatched types")
        );
        let node = "node:internal/modules\n  throw err;\nError: Cannot find module 'left-pad'\n    at Module._resolve";
        assert_eq!(
            first_error_line(node),
            Some("Error: Cannot find module 'left-pad'")
        );
        assert_eq!(first_error_line("all good\nno problems"), None);
    }
}
