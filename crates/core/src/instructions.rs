//! The two texts the hook gives an agent: the instruction line appended to the session brief and
//! the one-time reminder at Stop. The built-in defaults live here so the server and the client
//! agree; an owner or admin may replace either, globally or for one project.

/// The longest text the server stores for either field, in bytes.
pub const MAX_INSTRUCTION_BYTES: usize = 600;

/// Appended to the session brief.
pub const DEFAULT_SESSION_INSTRUCTION: &str = "Cairn is the only memory. Do not read or write built-in memory files. Call memory_search before deciding, debugging or acting on a name, tool, model, person or convention you do not know from this conversation. Call memory_save with a reason after decisions, fixes or corrections.";

/// The Stop reminder; `{files}` becomes the number of files the session edited.
pub const DEFAULT_STOP_NUDGE: &str = "You changed {files} file(s). If you learned something reusable, call memory_save once with items: 0-3 notes (decision/gotcha/rule/fact), each with a reason. Else stop.";

/// Appended to the Stop reminder, or sent alone, when the user corrected the agent.
/// `{correction}` is the quoted line. The text around that quote is at most 90 bytes.
pub const DEFAULT_CORRECTION_NUDGE: &str =
    "You were corrected (\"{correction}\"): save it as a rule if it holds beyond this task.";

/// A stop reminder with `{files}` replaced by the edited-files count.
pub fn fill_stop_nudge(template: &str, files: usize) -> String {
    template.replace("{files}", &files.to_string())
}

/// The correction clause with `{correction}` replaced by the stored line.
pub fn fill_correction_nudge(correction: &str) -> String {
    DEFAULT_CORRECTION_NUDGE.replace("{correction}", correction)
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
    fn default_requires_cairn_for_recall_and_saves() {
        assert_eq!(
            DEFAULT_SESSION_INSTRUCTION,
            "Cairn is the only memory. Do not read or write built-in memory files. Call memory_search before deciding, debugging or acting on a name, tool, model, person or convention you do not know from this conversation. Call memory_save with a reason after decisions, fixes or corrections."
        );
    }

    #[test]
    fn the_defaults_fit_and_the_placeholder_is_filled() {
        assert!(DEFAULT_SESSION_INSTRUCTION.len() <= 280);
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

    #[test]
    fn correction_clause_fits_budget() {
        let fixed = DEFAULT_CORRECTION_NUDGE.len() - "{correction}".len();
        assert!(fixed <= 90, "{fixed}");
        let quote = "No, never use npm here";
        let clause = fill_correction_nudge(quote);
        assert!(
            clause.contains(quote) && clause.contains("rule"),
            "{clause}"
        );
        assert!(clause.len() <= 90 + quote.len(), "{}", clause.len());
    }
}
