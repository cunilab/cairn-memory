//! The two texts the hook gives an agent: the instruction line appended to the session brief and
//! the one-time reminder at Stop. The built-in defaults live here so the server and the client
//! agree; an owner or admin may replace either, globally or for one project.

/// The longest text the server stores for either field, in bytes.
pub const MAX_INSTRUCTION_BYTES: usize = 600;

/// Appended to the session brief in a repository with `.cairn/project.json`.
pub const DEFAULT_SESSION_INSTRUCTION: &str = "Cairn is active for this project. Before deciding or debugging, memory_search; after a decision, fixed gotcha or finished task, memory_save with a reason.";

/// The Stop reminder; `{files}` becomes the number of files the session edited.
pub const DEFAULT_STOP_NUDGE: &str = "You changed {files} file(s). If you learned something reusable, call memory_save once with items: 0-3 notes (decision/gotcha/rule/fact), each with a reason. Else stop.";

/// A stop reminder with `{files}` replaced by the edited-files count.
pub fn fill_stop_nudge(template: &str, files: usize) -> String {
    template.replace("{files}", &files.to_string())
}

/// Checks one stored text: at most `MAX_INSTRUCTION_BYTES`, no control characters except a
/// newline. Returns the trimmed text; an empty one means "use the next level".
pub fn validate_instruction(field: &str, text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.len() > MAX_INSTRUCTION_BYTES {
        return Err(format!(
            "{field} is {} bytes; the limit is {MAX_INSTRUCTION_BYTES}",
            text.len()
        ));
    }
    if text.chars().any(|c| c.is_control() && c != '\n') {
        return Err(format!("{field} must not contain control characters"));
    }
    Ok(text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_fit_and_the_placeholder_is_filled() {
        assert!(DEFAULT_SESSION_INSTRUCTION.len() <= 200);
        assert!(DEFAULT_STOP_NUDGE.len() <= MAX_INSTRUCTION_BYTES);
        assert!(fill_stop_nudge(DEFAULT_STOP_NUDGE, 3).starts_with("You changed 3 file(s)."));
        assert_eq!(fill_stop_nudge("no placeholder", 3), "no placeholder");
    }

    #[test]
    fn validation_caps_bytes_and_trims() {
        assert_eq!(validate_instruction("f", "  hi \n"), Ok("hi".to_owned()));
        assert_eq!(validate_instruction("f", "   "), Ok(String::new()));
        assert!(validate_instruction("f", &"x".repeat(MAX_INSTRUCTION_BYTES)).is_ok());
        let long = validate_instruction("stop_nudge", &"é".repeat(301)).unwrap_err();
        assert_eq!(long, "stop_nudge is 602 bytes; the limit is 600");
        assert!(validate_instruction("f", "a\u{0}b").is_err());
        assert!(validate_instruction("f", "two\nlines").is_ok());
    }

    #[test]
    fn stop_text_fits_budget() {
        assert_eq!(
            DEFAULT_STOP_NUDGE,
            "You changed {files} file(s). If you learned something reusable, call memory_save once with items: 0-3 notes (decision/gotcha/rule/fact), each with a reason. Else stop."
        );
        assert!(
            DEFAULT_STOP_NUDGE.len() <= 170,
            "{}",
            DEFAULT_STOP_NUDGE.len()
        );
        let wrapped =
            serde_json::json!({"decision": "block", "reason": DEFAULT_STOP_NUDGE}).to_string();
        assert!(wrapped.len() <= 230, "{}", wrapped.len());
        assert_eq!(
            validate_instruction("stop_nudge", "off"),
            Ok("off".to_owned())
        );
    }
}
