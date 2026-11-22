//! MCP tool results: one channel, `content[0].text`, carrying the server's compact JSON.

use serde_json::{Value, json};

pub fn success(value: &Value) -> Value {
    json!({"content": [{"type": "text", "text": value.to_string()}]})
}

pub fn failure(message: &str) -> Value {
    json!({"isError": true, "content": [{"type": "text", "text": message}]})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_success_has_one_channel() {
        let result = success(&json!({"results": [], "more": 2}));
        assert_eq!(
            result.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["content"]
        );
        assert_eq!(result["content"][0]["text"], r#"{"more":2,"results":[]}"#);
    }

    #[test]
    fn present_failure_is_a_flagged_text() {
        let result = failure("reason: required for decision");
        assert_eq!(result["isError"], true);
        assert_eq!(
            result["content"][0]["text"],
            "reason: required for decision"
        );
    }
}
