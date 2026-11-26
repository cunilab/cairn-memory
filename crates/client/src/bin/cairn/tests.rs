use super::{TokenInput, token_input};

#[test]
fn token_input_falls_back_when_the_terminal_cannot_hide() {
    assert_eq!(token_input(false, || unreachable!()), TokenInput::Piped);
    assert_eq!(token_input(true, || true), TokenInput::Hidden);
    assert_eq!(token_input(true, || false), TokenInput::Visible);
}

#[test]
fn cli_usage_lists_only_kept_commands() {
    for gone in [
        "capture",
        "why-changed",
        "resume",
        "lesson",
        "export",
        "project-",
    ] {
        assert!(!super::USAGE.contains(gone), "{gone}");
    }
    for kept in [
        "setup", "init", "import", "update", "doctor", "mcp", "search", "get", "save", "feedback",
        "forget",
    ] {
        assert!(super::USAGE.contains(&format!("cairn {kept}")), "{kept}");
    }
}
