//! The hook payload and what each event captures (CM-SPEC-006 §1).

use super::readonly::{canonical_tool, is_read_only_tool, shell_command_is_readonly};
use cairn_core::capture::{CaptureKind, redact_sensitive_content};
use cairn_core::id::sha256_hex;
use serde_json::{Map, Value};

const PROMPT_MAX_BYTES: usize = 8 * 1024;
const EXCERPT_MAX_BYTES: usize = 1024;
const COMMAND_MAX_BYTES: usize = 1024;
const ERROR_LINE_MAX_BYTES: usize = 200;
const MARKER: &str = "\n[truncated]";
const WORK_TOOLS: [&str; 5] = ["Bash", "Edit", "Write", "MultiEdit", "apply_patch"];
const FILE_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "apply_patch"];

/// The longest prefix of `text` within `limit` bytes, on a character boundary.
pub fn truncate(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `text` within `limit` bytes; a longer text ends with a visible marker inside the limit.
pub fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let keep = limit.saturating_sub(MARKER.len());
    format!("{}{MARKER}", truncate(text, keep))
}

/// What the host sent on stdin: one JSON object.
pub struct Payload(Map<String, Value>);

impl Payload {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match serde_json::from_str::<Value>(raw) {
            Ok(Value::Object(map)) => Ok(Self(map)),
            _ => Err("hook input must be one JSON object".to_owned()),
        }
    }

    fn text(&self, key: &str) -> Option<&str> {
        self.0
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
    }

    pub fn session_id(&self) -> Option<&str> {
        self.text("session_id")
    }

    pub fn cwd(&self) -> Option<&str> {
        self.text("cwd")
    }

    pub fn tool(&self) -> Option<Tool<'_>> {
        let name = self.text("tool_name")?;
        Some(Tool {
            name: canonical_tool(name),
            input: self.0.get("tool_input").and_then(Value::as_object),
            response: self.0.get("tool_response"),
        })
    }

    /// `kind=prompt`: the redacted prompt, clipped to 8 KiB.
    pub fn prompt(&self) -> Option<Capture> {
        let prompt = self.text("prompt")?;
        Some(Capture::new(
            CaptureKind::Prompt,
            text_of(prompt, PROMPT_MAX_BYTES),
        ))
    }

    /// `kind=assistant`; the caller decides whether `CAIRN_CAPTURE_ASSISTANT` allows it.
    pub fn assistant(&self) -> Option<Capture> {
        let message = self.text("last_assistant_message")?;
        Some(Capture::new(
            CaptureKind::Assistant,
            text_of(message, PROMPT_MAX_BYTES),
        ))
    }
}

fn text_of(text: &str, limit: usize) -> String {
    clip(&redact_sensitive_content(text), limit)
}

/// One captured event before it becomes a typed `/observe` body.
pub struct Capture {
    pub kind: CaptureKind,
    pub ok: Option<bool>,
    pub content: String,
    pub files: Vec<String>,
    pub commands: Vec<String>,
}

impl Capture {
    fn new(kind: CaptureKind, content: String) -> Self {
        Self {
            kind,
            ok: None,
            content,
            files: Vec::new(),
            commands: Vec::new(),
        }
    }

    /// `hk_` + 24 hex of sha256(session, kind, content): a retry keeps its id, and the server
    /// drops a second identical capture of the session by the same content anyway.
    pub fn event_id(&self, session_id: &str) -> String {
        let hash = sha256_hex(&[session_id, self.kind.as_str(), &self.content]);
        format!("hk_{}", &hash[..24])
    }

    /// First 16 hex of sha256(kind, content): the client-side repeat check.
    pub fn content_hash(&self) -> String {
        sha256_hex(&[self.kind.as_str(), &self.content])[..16].to_owned()
    }
}

/// The files an `apply_patch` text touches, each with its own lines (header excluded).
fn patch_sections(patch: &str) -> Vec<(String, String)> {
    const HEADERS: [&str; 3] = ["*** Add File: ", "*** Update File: ", "*** Delete File: "];
    let mut sections: Vec<(String, Vec<&str>)> = Vec::new();
    for line in patch.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(path) = HEADERS.iter().find_map(|header| line.strip_prefix(header)) {
            sections.push((path.trim().to_owned(), Vec::new()));
        } else if line.starts_with("*** End Patch") {
            break;
        } else if let Some((_, body)) = sections.last_mut() {
            body.push(line);
        }
    }
    sections
        .into_iter()
        .filter(|(path, _)| !path.is_empty())
        .map(|(path, body)| (path, body.join("\n")))
        .collect()
}

/// A tool call, with the tool name canonical.
pub struct Tool<'a> {
    pub name: String,
    input: Option<&'a Map<String, Value>>,
    response: Option<&'a Value>,
}

impl Tool<'_> {
    fn input_text(&self, key: &str) -> Option<&str> {
        self.input?
            .get(key)?
            .as_str()
            .filter(|text| !text.is_empty())
    }

    fn command_text(&self) -> Option<&str> {
        self.input_text("command")
            .or_else(|| self.input_text("cmd"))
    }

    pub fn is_read_only(&self) -> bool {
        is_read_only_tool(&self.name)
            || (self.name == "Bash" && self.command_text().is_some_and(shell_command_is_readonly))
    }

    /// Edits and non-read-only shell: the calls that make a session "worked".
    pub fn is_work(&self) -> bool {
        WORK_TOOLS.contains(&self.name.as_str()) && !self.is_read_only()
    }

    pub fn is_file_tool(&self) -> bool {
        FILE_TOOLS.contains(&self.name.as_str())
    }

    /// The file a read tool opened: never captured, but its topic notes are worth showing.
    pub fn read_path(&self) -> Option<&str> {
        (self.name == "Read").then(|| self.file_path()).flatten()
    }

    /// The structured path argument; the file itself is never read.
    fn file_path(&self) -> Option<&str> {
        ["file_path", "filePath", "path"]
            .iter()
            .find_map(|key| self.input_text(key))
    }

    /// Every edited file with its new text: the structured path, else the `*** Add|Update|Delete
    /// File:` headers of an `apply_patch` text (Codex sends the patch as `command`).
    pub fn edits(&self) -> Vec<(String, String)> {
        if let Some(path) = self.file_path() {
            return vec![(path.to_owned(), self.new_text())];
        }
        ["command", "patch", "patchText", "input", "cmd"]
            .iter()
            .find_map(|key| self.input_text(key))
            .map(patch_sections)
            .unwrap_or_default()
    }

    fn new_text(&self) -> String {
        match self.input.and_then(|input| input.get("edits")) {
            Some(Value::Array(edits)) => edits
                .iter()
                .filter_map(|edit| edit.get("new_string")?.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => [
                "new_string",
                "newString",
                "new_str",
                "content",
                "patch",
                "patchText",
                "input",
                "command",
            ]
            .iter()
            .find_map(|key| self.input_text(key))
            .unwrap_or_default()
            .to_owned(),
        }
    }

    /// `"<Tool> <path>\n"` plus the new text, redacted then clipped to 1 KiB.
    pub fn edit(&self, path: &str, new_text: &str) -> Capture {
        let mut capture = Capture::new(
            CaptureKind::Edit,
            format!(
                "{} {path}\n{}",
                self.name,
                text_of(new_text, EXCERPT_MAX_BYTES)
            ),
        );
        capture.files = vec![path.to_owned()];
        capture
    }

    /// `None` for a read-only or non-shell call. A failed command keeps only the command, the
    /// exit code and the first error line.
    pub fn command(&self) -> Option<Capture> {
        if self.name != "Bash" || self.is_read_only() {
            return None;
        }
        let command = text_of(self.command_text()?, COMMAND_MAX_BYTES);
        let ok = self.ok();
        let mut content = command.clone();
        if ok == Some(false) {
            match self.exit_code() {
                Some(code) => content.push_str(&format!("\nexit {code}")),
                None => content.push_str("\nfailed"),
            }
            if let Some(line) = self.first_error_line() {
                content.push_str(&format!(": {line}"));
            }
        }
        let mut capture = Capture::new(CaptureKind::Command, content);
        capture.ok = ok;
        capture.commands = vec![command];
        Some(capture)
    }

    fn response_field(&self, key: &str) -> Option<&Value> {
        self.response?.get(key)
    }

    fn exit_code(&self) -> Option<i64> {
        self.response_field("exit_code")?.as_i64()
    }

    /// `exit_code == 0` when present, else neither `is_error` nor `interrupted` set; else unknown.
    fn ok(&self) -> Option<bool> {
        if let Some(code) = self.exit_code() {
            return Some(code == 0);
        }
        let flags = ["is_error", "interrupted"].map(|key| self.response_field(key));
        if flags.iter().all(Option::is_none) {
            return None;
        }
        Some(flags.iter().all(|flag| *flag != Some(&Value::Bool(true))))
    }

    fn first_error_line(&self) -> Option<String> {
        ["stderr", "stdout"]
            .iter()
            .filter_map(|key| self.response_field(key)?.as_str())
            .chain(self.response.and_then(Value::as_str))
            .find_map(|text| text.lines().map(str::trim).find(|line| !line.is_empty()))
            .map(|line| truncate(&redact_sensitive_content(line), ERROR_LINE_MAX_BYTES).to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload(value: Value) -> Payload {
        Payload::parse(&value.to_string()).unwrap()
    }

    #[test]
    fn clip_keeps_a_marker_inside_the_limit_on_a_character_boundary() {
        assert_eq!(clip("short", 20), "short");
        let clipped = clip(&"é".repeat(100), 40);
        assert!(
            clipped.len() <= 40 && clipped.ends_with(MARKER),
            "{clipped}"
        );
        assert!(clip("x", 3).len() <= 12);
    }

    #[test]
    fn ids_follow_session_kind_and_content() {
        let capture = Capture::new(CaptureKind::Command, "cargo test".into());
        let id = capture.event_id("s");
        assert_eq!(id, capture.event_id("s"));
        assert_ne!(id, capture.event_id("other"));
        assert_ne!(
            id,
            Capture::new(CaptureKind::Prompt, "cargo test".into()).event_id("s")
        );
        assert!(id.starts_with("hk_") && id.len() == 27);
    }

    #[test]
    fn patch_headers_give_one_section_per_file() {
        let patch = "*** Begin Patch\r\n*** Update File: a.rs\r\n@@\r\n-x\r\n+y\r\n*** Add File:  dir/b.rs \n+new\n*** Delete File: c.rs\n*** End Patch\n*** Update File: ignored.rs\n";
        assert_eq!(
            patch_sections(patch),
            [
                ("a.rs".to_owned(), "@@\n-x\n+y".to_owned()),
                ("dir/b.rs".to_owned(), "+new".to_owned()),
                ("c.rs".to_owned(), String::new()),
            ]
        );
        assert!(patch_sections("no headers here").is_empty());
    }

    #[test]
    fn a_failed_command_keeps_command_exit_and_first_error_line() {
        let p = payload(
            json!({"tool_name": "Bash", "tool_input": {"command": "cargo test"},
            "tool_response": {"exit_code": 101, "stderr": "\n  error[E0432]: nope\nmore", "stdout": "x"}}),
        );
        let capture = p.tool().unwrap().command().unwrap();
        assert_eq!(capture.content, "cargo test\nexit 101: error[E0432]: nope");
        assert_eq!(capture.ok, Some(false));
        assert_eq!(capture.commands, ["cargo test"]);
    }

    #[test]
    fn ok_reads_exit_code_then_flags_then_is_unknown() {
        let ok = |response: Value| {
            payload(json!({"tool_name": "Bash", "tool_input": {"command": "make"}, "tool_response": response}))
                .tool()
                .unwrap()
                .command()
                .unwrap()
                .ok
        };
        assert_eq!(ok(json!({"exit_code": 0})), Some(true));
        assert_eq!(
            ok(json!({"is_error": false, "interrupted": false})),
            Some(true)
        );
        assert_eq!(ok(json!({"interrupted": true})), Some(false));
        assert_eq!(ok(json!({"stdout": "fine"})), None);
    }

    #[test]
    fn edit_content_is_redacted_then_clipped() {
        let secret = "ghp_".to_owned() + &"a".repeat(40);
        let p = payload(
            json!({"tool_name": "Edit", "tool_input": {"file_path": "a.rs",
            "new_string": format!("token={secret}\n{}", "x".repeat(3000))}}),
        );
        let tool = p.tool().unwrap();
        let (path, text) = tool.edits().remove(0);
        assert_eq!(path, "a.rs");
        let capture = tool.edit("a.rs", &text);
        assert!(capture.content.starts_with("Edit a.rs\n"));
        assert!(!capture.content.contains(&secret));
        assert!(capture.content.len() <= "Edit a.rs\n".len() + EXCERPT_MAX_BYTES);
        assert!(capture.content.ends_with(MARKER));
        assert_eq!(capture.files, ["a.rs"]);
    }

    #[test]
    fn work_tools_exclude_read_only_commands() {
        let work = |name: &str, command: &str| {
            payload(json!({"tool_name": name, "tool_input": {"command": command}}))
                .tool()
                .unwrap()
                .is_work()
        };
        assert!(work("Bash", "cargo build"));
        assert!(!work("Bash", "git status"));
        assert!(!work("Read", "x"));
        assert!(work("bash", "make"));
    }
}
