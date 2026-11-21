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

/// A member by name, or an array item by index (written as digits).
fn child<'a>(node: &'a Node, segment: &str) -> Option<&'a Node> {
    if node.value.is_array() {
        node.items.get(segment.parse::<usize>().ok()?)
    } else {
        node.member(segment)
    }
}

fn find<'a>(root: &'a Node, path: &[&str]) -> Option<&'a Node> {
    path.iter()
        .try_fold(root, |node, segment| child(node, segment))
}

fn child_count(node: &Node) -> usize {
    node.members.len() + node.items.len()
}

/// The whitespace that starts the line holding `at`.
fn line_indent(text: &str, at: usize) -> &str {
    let start = text[..at].rfind('\n').map_or(0, |newline| newline + 1);
    let line = &text[start..at];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

fn render(value: &Value, indent: &str) -> String {
    serde_json::to_string_pretty(value)
        .expect("a JSON value serializes")
        .replace('\n', &format!("\n{indent}"))
}

/// Adds `value` (under `key` for an object) after the last child, on its own line when the last
/// child is on one. An empty `{}` or `[]` is opened onto lines.
fn insert(text: &str, container: &Node, key: Option<&str>, value: &Value) -> String {
    let base = line_indent(text, container.start);
    let entry = |indent: &str| {
        let value = render(value, indent);
        match key {
            Some(key) => format!("{}: {value}", Value::from(key)),
            None => value,
        }
    };
    let mut out = text.to_owned();
    let last = container
        .members
        .last()
        .map(|member| &member.1)
        .or(container.items.last());
    let Some(last) = last else {
        let inner = format!("{base}  ");
        let (open, close) = if container.value.is_array() {
            ('[', ']')
        } else {
            ('{', '}')
        };
        let body = format!("{open}\n{inner}{}\n{base}{close}", entry(&inner));
        out.replace_range(container.start..container.end, &body);
        return out;
    };
    let own_line = text[..last.key_start]
        .trim_end_matches([' ', '\t'])
        .ends_with('\n');
    let indent = if own_line {
        line_indent(text, last.key_start)
    } else {
        ""
    };
    let separator = if own_line {
        format!("\n{indent}")
    } else {
        " ".to_owned()
    };
    match last.comma {
        Some(comma) => out.insert_str(comma.1, &format!("{separator}{},", entry(indent))),
        None => out.insert_str(last.end, &format!(",{separator}{}", entry(indent))),
    }
    out
}

/// Sets the value at `path` (member names, array indexes as digits): replaces it in place, or
/// adds it as a new last member, creating the missing objects on the way. Every other byte stays.
pub fn put(text: &str, path: &[&str], value: &Value) -> Result<String, String> {
    let root = read(text)?;
    if let Some(node) = find(&root, path) {
        if node.value == *value {
            return Ok(text.to_owned());
        }
        let mut out = text.to_owned();
        out.replace_range(
            node.start..node.end,
            &render(value, line_indent(text, node.start)),
        );
        return Ok(out);
    }
    let (mut node, mut depth) = (&root, 0);
    while let Some(next) = child(node, path[depth]) {
        if !next.value.is_object() {
            return Err(format!("{} must be an object", path[depth]));
        }
        (node, depth) = (next, depth + 1);
    }
    let wrapped = path[depth + 1..].iter().rev().fold(
        value.clone(),
        |inner, key| serde_json::json!({ *key: inner }),
    );
    Ok(insert(text, node, Some(path[depth]), &wrapped))
}

/// Appends `value` to the array at `path`, creating it when it is missing.
pub fn push(text: &str, path: &[&str], value: &Value) -> Result<String, String> {
    let root = read(text)?;
    match find(&root, path) {
        Some(array) if array.value.is_array() => Ok(insert(text, array, None, value)),
        Some(_) => Err(format!("{} must be an array", path.last().unwrap_or(&""))),
        None => put(text, path, &Value::Array(vec![value.clone()])),
    }
}

/// Removes the member or item at `path` with its separator and line; a missing path changes
/// nothing. A container this would leave empty goes too (never the root), so removing what was
/// added restores the file.
pub fn delete(text: &str, path: &[&str]) -> Result<String, String> {
    let root = read(text)?;
    let mut chain = vec![&root];
    for segment in path {
        match child(chain[chain.len() - 1], segment) {
            Some(next) => chain.push(next),
            None => return Ok(text.to_owned()),
        }
    }
    let mut level = path.len();
    while level > 1 && child_count(chain[level - 1]) == 1 {
        level -= 1;
    }
    let node = chain[level];
    let before = text[..node.key_start].trim_end_matches([' ', '\t']);
    let line_start = before
        .strip_suffix("\r\n")
        .or_else(|| before.strip_suffix('\n'))
        .map_or(node.key_start, str::len);
    let (start, end) = match (node.comma, node.previous_comma) {
        (Some(comma), _) => (line_start, comma.1),
        (None, Some(previous)) => (previous.0, node.end),
        (None, None) => (line_start, node.end),
    };
    let mut out = text.to_owned();
    out.replace_range(start..end, "");
    Ok(out)
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
    fn put_push_and_delete_edit_by_span_and_round_trip() {
        let text = "{\n\t\"b\": 1e3,\n\t\"a\": {\"x\": [1, 2]}\n}\n";
        let added = put(text, &["m", "cairn"], &json!({"k": 1})).unwrap();
        assert!(added.starts_with("{\n\t\"b\": 1e3,\n\t\"a\": {\"x\": [1, 2]},\n\t\"m\": {"));
        assert_eq!(read(&added).unwrap().value["m"]["cairn"]["k"], 1);
        let pushed = push(&added, &["a", "x"], &json!(3)).unwrap();
        assert!(pushed.contains("[1, 2, 3]"), "{pushed}");
        let replaced = put(&pushed, &["a", "x", "0"], &json!("one")).unwrap();
        assert!(replaced.contains("[\"one\", 2, 3]"), "{replaced}");
        // Removing what was added gives back the original bytes.
        assert_eq!(delete(&added, &["m", "cairn"]).unwrap(), text);
        let popped = delete(&pushed, &["a", "x", "2"]).unwrap();
        assert_eq!(delete(&popped, &["m"]).unwrap(), text);
        // Nothing to do: the very same text.
        assert_eq!(delete(text, &["m", "cairn"]).unwrap(), text);
        assert_eq!(put(text, &["a", "x"], &json!([1, 2])).unwrap(), text);
        // An empty object opens onto lines; a scalar in the way is refused.
        assert_eq!(
            put("{}\n", &["h"], &json!({"e": []})).unwrap(),
            "{\n  \"h\": {\n    \"e\": []\n  }\n}\n"
        );
        assert!(put(text, &["b", "c"], &json!(1)).is_err());
        assert!(push(text, &["b"], &json!(1)).is_err());
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
