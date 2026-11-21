//! A JSONC reader that keeps token spans, so removing one entry leaves every other byte,
//! comment and separator of the user's OpenCode config untouched (port of `jsonc()`).

use regex::Regex;
use serde_json::{Map, Value};
use std::sync::OnceLock;

type Span = (usize, usize);

pub struct Node {
    pub value: Value,
    pub start: usize,
    pub end: usize,
    pub members: Vec<(String, Node)>,
    pub items: Vec<Node>,
    pub key_start: usize,
    pub comma: Option<Span>,
    pub previous_comma: Option<Span>,
}

impl Node {
    pub fn member(&self, key: &str) -> Option<&Node> {
        self.members
            .iter()
            .find(|(name, _)| name == key)
            .map(|m| &m.1)
    }
}

fn tokenize(text: &str) -> Result<Vec<(&str, usize, usize)>, String> {
    static TOKEN: OnceLock<Regex> = OnceLock::new();
    let token = TOKEN.get_or_init(|| {
        Regex::new(r#""(?:\\.|[^"\\])*"|//[^\r\n]*|(?s:/\*.*?\*/)|[{}\[\],:]|[^\s{}\[\],:]+"#)
            .expect("static pattern")
    });
    let (mut tokens, mut end) = (Vec::new(), 0);
    for found in token.find_iter(text) {
        if !text[end..found.start()].trim().is_empty() {
            return Err("invalid JSONC syntax".into());
        }
        let value = found.as_str();
        if value.starts_with("/*") && !value.ends_with("*/") {
            return Err("unclosed JSONC comment".into());
        }
        if !value.starts_with("//") && !value.starts_with("/*") {
            tokens.push((value, found.start(), found.end()));
        }
        end = found.end();
    }
    if !text[end..].trim().is_empty() {
        return Err("invalid JSONC syntax".into());
    }
    Ok(tokens)
}

fn parse(
    tokens: &[(&str, usize, usize)],
    mut index: usize,
    depth: usize,
) -> Result<(Node, usize), String> {
    if index >= tokens.len() || depth > 64 {
        return Err("invalid or too deeply nested JSONC".into());
    }
    let (token, start, _) = tokens[index];
    let plain = |value, start, end| Node {
        value,
        start,
        end,
        members: Vec::new(),
        items: Vec::new(),
        key_start: start,
        comma: None,
        previous_comma: None,
    };
    if token != "{" && token != "[" {
        let value = serde_json::from_str(token).map_err(|_| "invalid JSONC value")?;
        return Ok((plain(value, start, tokens[index].2), index + 1));
    }
    let (object, closing) = (token == "{", if token == "{" { "}" } else { "]" });
    let (mut map, mut list) = (Map::new(), Vec::new());
    let (mut members, mut items) = (Vec::new(), Vec::new());
    index += 1;
    let mut previous = None;
    while index < tokens.len() && tokens[index].0 != closing {
        let key_start = tokens[index].1;
        let mut key = String::new();
        if object {
            key = serde_json::from_str(tokens[index].0)
                .map_err(|_| "invalid or duplicate JSONC key")?;
            if map.contains_key(&key) || tokens.get(index + 1).is_none_or(|t| t.0 != ":") {
                return Err("invalid or duplicate JSONC key".into());
            }
            index += 2;
        }
        let (mut child, next) = parse(tokens, index, depth + 1)?;
        (child.key_start, child.previous_comma, index) = (key_start, previous, next);
        if index < tokens.len() && tokens[index].0 == "," {
            child.comma = Some((tokens[index].1, tokens[index].2));
            previous = child.comma;
            index += 1;
        } else if index >= tokens.len() || tokens[index].0 != closing {
            return Err("missing JSONC separator".into());
        }
        if object {
            map.insert(key.clone(), child.value.clone());
            members.push((key, child));
        } else {
            list.push(child.value.clone());
            items.push(child);
        }
    }
    if index >= tokens.len() {
        return Err("unclosed JSONC container".into());
    }
    let value = if object {
        Value::Object(map)
    } else {
        Value::Array(list)
    };
    let mut node = plain(value, start, tokens[index].2);
    (node.members, node.items) = (members, items);
    Ok((node, index + 1))
}

/// The root must be one object.
pub fn read(text: &str) -> Result<Node, String> {
    let tokens = tokenize(text)?;
    let (root, used) = parse(&tokens, 0, 0)?;
    if used != tokens.len() || !root.value.is_object() {
        return Err("config must be one JSONC object".into());
    }
    Ok(root)
}

fn cut(mut text: String, mut cuts: Vec<Span>) -> String {
    cuts.sort_unstable();
    cuts.dedup();
    for (start, end) in cuts.into_iter().rev() {
        text.replace_range(start..end, "");
    }
    text
}

/// Removes `mcp.servers.cairn` with its separator. A trailing comma may remain only when the
/// file allows it (`.jsonc`); otherwise the previous comma goes too.
pub fn remove_cairn(text: &str, root: &Node, allow_trailing: bool) -> Result<String, String> {
    let mut node = root;
    for key in ["mcp", "servers", "cairn"] {
        match node.member(key) {
            Some(next) => node = next,
            None => return Ok(text.to_owned()),
        }
    }
    if !super::owned(&node.value) {
        return Err(super::UNMARKED.into());
    }
    let mut cuts = vec![(node.key_start, node.end)];
    match (node.comma, node.previous_comma) {
        (Some(comma), _) => cuts.push(comma),
        (None, Some(previous)) if !allow_trailing => cuts.push(previous),
        _ => {}
    }
    Ok(cut(text.to_owned(), cuts))
}

/// The retired single-file plugin entry only; another package using the same option stays.
fn retired_plugin(entry: &Value) -> bool {
    let package = entry.get("package").and_then(Value::as_str);
    let options = entry.get("options").and_then(Value::as_object);
    matches!((package, options), (Some(package), Some(options))
        if options.get("cairn") == Some(&Value::from("continuity"))
            && package.replace('\\', "/").trim_end_matches('/').ends_with("/opencode/plugin/cairn.js"))
}

pub fn remove_retired_plugins(text: &str, root: &Node) -> Result<String, String> {
    let Some(plugins) = root.member("plugins") else {
        return Ok(text.to_owned());
    };
    if !plugins.value.is_array() {
        return Err("plugins must be an array".into());
    }
    let mut cuts = Vec::new();
    for child in plugins.items.iter().filter(|c| retired_plugin(&c.value)) {
        cuts.push((child.start, child.end));
        cuts.extend(child.comma.or(child.previous_comma));
    }
    Ok(cut(text.to_owned(), cuts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn marked() -> Value {
        json!({"environment": {"CAIRN_SETUP_MANAGED": "1"}, "command": ["cairn", "mcp"]})
    }

    #[test]
    fn jsonc_edits_keep_other_bytes() {
        let text = r#"{
 // before
 "mcp": { "servers": {
    "other": {"command": ["{ // /* */ }", "\\\""], "nested": {"ok": true}},
    /* keep this */ "cairn": {"environment":{"CAIRN_SETUP_MANAGED":"1"},"command":["cairn", "mcp"],}, // keep that
    "last": {"url":"https://x.test/a//b/*c*/"},
 }}, // end
}"#;
        let node = read(text).unwrap();
        let result = remove_cairn(text, &node, true).unwrap();
        assert!(!result.contains("\"cairn\""));
        for comment in ["// before", "/* keep this */", "// keep that", "// end"] {
            assert!(result.contains(comment), "{comment}");
        }
        let servers = |n: &Node| {
            n.member("mcp")
                .unwrap()
                .member("servers")
                .unwrap()
                .value
                .clone()
        };
        let (before, after) = (servers(&node), servers(&read(&result).unwrap()));
        assert_eq!(after["other"], before["other"]);
        assert_eq!(after["last"], before["last"]);
        assert!(after.get("cairn").is_none());
        // The change is exactly the owned key/value and its separator.
        let member = node
            .member("mcp")
            .unwrap()
            .member("servers")
            .unwrap()
            .member("cairn")
            .unwrap();
        let comma = member.comma.unwrap();
        let expected = format!(
            "{}{}{}",
            &text[..member.key_start],
            &text[member.end..comma.0],
            &text[comma.1..]
        );
        assert_eq!(result, expected);

        // Last, first, only and missing entries all stay valid JSON.
        for items in [
            json!({"other": {}, "cairn": marked()}),
            json!({"cairn": marked(), "other": {}}),
            json!({"cairn": marked()}),
            json!({"other": {}}),
        ] {
            let text = json!({"mcp": {"servers": items}}).to_string();
            let result = remove_cairn(&text, &read(&text).unwrap(), false).unwrap();
            let mut expected = items.as_object().unwrap().clone();
            expected.remove("cairn");
            let parsed: Value = serde_json::from_str(&result).unwrap();
            assert_eq!(parsed["mcp"]["servers"], Value::Object(expected));
            if items.get("cairn").is_none() {
                assert_eq!(result, text);
            }
        }
    }

    #[test]
    fn duplicate_and_malformed_input_fail_closed() {
        for text in [
            "{\"a\":1,\"a\":2}",
            "{/* unclosed",
            "{\"a\":}",
            "{\"a\":1} trailing",
            "{} /*unclosed",
        ] {
            assert!(read(text).is_err(), "{text}");
        }
    }

    #[test]
    fn an_unmarked_same_name_entry_is_refused_even_on_remove() {
        let text = r#"{"mcp":{"servers":{"cairn":{"command":["my-server"]}}}}"#;
        let error = remove_cairn(text, &read(text).unwrap(), false).unwrap_err();
        assert!(error.contains("unmarked"), "{error}");
    }

    #[test]
    fn only_the_retired_plugin_entry_is_removed() {
        let text = r#"{ "plugins": ["a", {"package":"kept","options":{"cairn":"continuity"}}, {"package":"/tmp/opencode/plugin/cairn.js","options":{"cairn":"continuity"}}] }"#;
        let result = remove_retired_plugins(text, &read(text).unwrap()).unwrap();
        assert!(result.contains("kept") && result.contains("\"a\""));
        assert!(!result.contains("plugin/cairn.js"));
        assert!(read(&result).is_ok());
    }
}
