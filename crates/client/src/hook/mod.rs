//! `cairn hook <Event> [--agent a] [--config p] [--cairn-managed]`: the capture and injection
//! hook of every agent host (docs/reference/hook.md#events). The host's event arrives as JSON on stdin; stdout
//! carries only an injection reply or a native memory write decision. A failure writes one
//! stderr line and still exits 0.

mod event;
pub(crate) mod git;
mod inject;
mod outbox;
mod readonly;
mod state;

use crate::{Client, Config, Error, claude_memory, selected_config_path, update, write_private};
use cairn_core::account::ClientKind;
use cairn_core::capture::{
    CaptureKind, ChatContext, WorkContext, redact_sensitive_content, redact_work_context,
};
use cairn_core::id::{new_id, now_seconds, sha256_hex};
use cairn_core::instructions::{
    DEFAULT_CHAT_INSTRUCTION, DEFAULT_SESSION_INSTRUCTION, DEFAULT_STOP_NUDGE,
    fill_correction_nudge, fill_stop_nudge,
};
use cairn_core::process::distill::{
    HANDOFF_FILES_MAX, first_correction, has_typed_line, is_test_command,
};
use cairn_core::process::entities::{ENTITY_CAP, is_symbol, normalize_command};
use cairn_core::retrieve::command::error_tail;
use cairn_core::retrieve::prompt::{PROMPT_QUERY_MAX_BYTES, should_recall};
use event::{Capture, Payload};
use serde_json::{Value, json};
use state::{Locked, SessionState};
use std::cell::OnceCell;
use std::path::{Path, PathBuf};
use std::time::Duration;

const OBSERVE: &str = "/cairn-memory/observe";
const RECALL_TIMEOUT: Duration = Duration::from_secs(1);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
/// After a request got no answer, further requests are skipped (captures go straight to the
/// outbox) for this long, so a dead network costs one timeout instead of one per hook call.
const DOWN_SECONDS: i64 = 30;
/// The first server version that keeps chat memory separate (`context.chat`).
const CHAT_SCOPE_SERVER: update::Version = (0, 1, 10);
const NO_PROJECT_HINT: &str =
    "Cairn: no project here (personal memory). Run cairn init to share with a project.";
/// Already-dirty paths recorded at SessionStart.
const BASELINE_MAX: usize = 200;

/// Dirty paths that were clean at SessionStart, then tool edits, capped at `HANDOFF_FILES_MAX`.
fn changed_paths(state: &SessionState, cwd: &std::path::Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut push = |path: &str| {
        if !path.is_empty()
            && files.len() < HANDOFF_FILES_MAX
            && !files.iter().any(|seen| seen == path)
        {
            files.push(path.to_owned());
        }
    };
    let limit = state.baseline_dirty.len() + HANDOFF_FILES_MAX;
    for path in git::dirty_files(cwd, limit) {
        if !state.baseline_dirty.iter().any(|seen| seen == &path) {
            push(&path);
        }
    }
    for path in &state.edited {
        push(path);
    }
    files
}

/// `cairn save ...` (any path to the binary): a save through the CLI instead of the MCP tool.
fn is_cairn_save(command: &str) -> bool {
    let mut words = command.split_whitespace();
    let program = words
        .next()
        .map(|w| w.rsplit(['/', '\\']).next().unwrap_or(w));
    matches!(program, Some("cairn" | "cairn.exe")) && words.next() == Some("save")
}

struct Options {
    event: String,
    agent: String,
    config: Option<PathBuf>,
    managed: bool,
    keep_native_memory: bool,
}

impl Options {
    fn parse(args: &[String]) -> Result<Self, Error> {
        let (event, rest) = args
            .split_first()
            .ok_or_else(|| Error::Input("hook needs an event name".to_owned()))?;
        let mut options = Self {
            event: event.clone(),
            agent: String::new(),
            config: None,
            managed: false,
            keep_native_memory: false,
        };
        let mut rest = rest.iter();
        while let Some(flag) = rest.next() {
            let mut value = || {
                rest.next()
                    .ok_or_else(|| Error::Input(format!("{flag} needs a value")))
            };
            match flag.as_str() {
                "--agent" => options.agent = value()?.clone(),
                "--config" => options.config = Some(PathBuf::from(value()?)),
                "--cairn-managed" => options.managed = true,
                crate::setup::KEEP_NATIVE_MEMORY
                    if matches!(options.event.as_str(), "SessionStart" | "PreToolUse") =>
                {
                    options.keep_native_memory = true;
                }
                other => return Err(Error::Input(format!("unexpected hook option {other:?}"))),
            }
        }
        Ok(options)
    }

    /// A managed hook lets a non-empty `CAIRN_CONFIG` replace the setup-time `--config`; an
    /// unmanaged `--config` wins over `CAIRN_CONFIG`.
    fn config_path(&self) -> Result<PathBuf, Error> {
        let chosen = std::env::var("CAIRN_CONFIG").unwrap_or_default();
        match (&self.config, self.managed && !chosen.trim().is_empty()) {
            (Some(path), false) => Ok(path.clone()),
            _ => selected_config_path(),
        }
    }
}

/// Returns the exit code. `args` is everything after `hook`.
pub fn run(args: &[String]) -> Result<i32, Error> {
    let options = Options::parse(args)?;
    // Grok imports Claude's hooks. Its native Cairn hooks own capture when installed.
    if options.agent == "claude"
        && options.managed
        && std::env::var_os("GROK_HOOK_EVENT").is_some()
        && std::env::home_dir().is_some_and(|home| crate::setup::grok::hooks_installed(&home))
    {
        return Ok(0);
    }
    let handle = session(&options);
    if let Err(message) = handle {
        eprintln!("cairn hook: {message}");
    }
    Ok(0)
}

fn guard_native_memory(payload: &Payload, tool: &event::Tool<'_>) {
    let key = match payload.text("tool_name") {
        Some("Write" | "Edit" | "MultiEdit") => "file_path",
        Some("NotebookEdit") => "notebook_path",
        _ => return,
    };
    if let Some(path) = tool.input_text(key)
        && claude_memory::is_memory_path(Path::new(path), payload.cwd().map(Path::new))
    {
        println!(
            "{}",
            json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "Claude memory files are replaced by Cairn; save this with memory_save instead."
            }})
        );
    }
}

fn session(options: &Options) -> Result<(), String> {
    let raw = std::io::read_to_string(std::io::stdin()).map_err(|_| "cannot read hook input")?;
    let mut payload = Payload::parse(&raw)?;
    if options.agent == "hermes" {
        payload.normalize_hermes();
    }
    let chat = if options.agent == "hermes" {
        payload
            .value("chat_context")
            .map(|value| serde_json::from_value::<ChatContext>(value.clone()))
            .transpose()
            .map_err(|_| "invalid Hermes chat context")?
    } else {
        None
    };
    if options.event == "PreToolUse" {
        // A running Hermes plugin from before the update may still call this and discard stdout.
        if options.agent == "hermes" {
            return Ok(());
        }
        let Some(tool) = payload.tool() else {
            return Ok(());
        };
        if tool.shell_command().is_none() {
            if options.agent == "claude" && !options.keep_native_memory {
                guard_native_memory(&payload, &tool);
            }
            return Ok(());
        }
    } else {
        crate::warn_ignored_legacy_config();
    }
    let path = options.config_path().map_err(|e| e.to_string())?;
    let config = match Config::load_from(&path) {
        Ok(config) => config,
        // Not set up here: the hook is simply off.
        Err(_) if !path.exists() => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if config
        .token
        .chars()
        .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("device credential must be one header token".to_owned());
    }
    let cwd = match payload.cwd() {
        Some(cwd) => PathBuf::from(cwd),
        None => std::env::current_dir().map_err(|e| format!("no working folder: {e}"))?,
    };
    let top = chat.is_none().then(|| git::top_level(&cwd)).flatten();
    if chat.is_none() && git::ignored(&cwd, top.as_deref()) {
        return Ok(());
    }
    let session_id = payload
        .session_id()
        .ok_or("hook payload requires session_id")?
        .to_owned();
    let config_dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let key = state::key(&config.url, &config.token, &options.agent, &session_id);
    let hook = Hook {
        event: &options.event,
        agent: &options.agent,
        config,
        config_dir,
        chat,
        key,
        session_id,
        cwd,
        top,
        context: OnceCell::new(),
        check: OnceCell::new(),
        payload,
    };
    let chat_tool = matches!(hook.event, "ChatSearch" | "ChatSave" | "ChatSkill");
    // Fail closed: a server older than 0.1.10 ignores `context.chat` and would treat chat
    // content as ordinary personal memory, so chat memory stays off until support is confirmed.
    if hook.chat.is_some() && !hook.chat_scope_supported() {
        if chat_tool {
            println!(
                "{}",
                json!({"error": "Cairn chat memory is off: this server does not keep chats separate."})
            );
        }
        return Ok(());
    }
    if chat_tool {
        if hook.chat.is_none() {
            return Err("Hermes chat tools require chat context".into());
        }
        let reply = hook.chat_tool().unwrap_or_else(|error| {
            let message = match error {
                Error::Input(message)
                | Error::Status {
                    status: 400 | 404 | 409,
                    message,
                } => redact_sensitive_content(&message),
                _ => "Cairn could not complete this chat request.".into(),
            };
            json!({"error": message})
        });
        println!("{reply}");
        return Ok(());
    }
    let text = hook.handle()?;
    if let Some(reply) = inject::output(hook.agent, hook.event, text.as_deref()) {
        println!("{reply}");
    }
    hook.report_repo_state();
    Ok(())
}

struct Hook<'a> {
    event: &'a str,
    /// Sent in the user agent so the console knows the device's host.
    agent: &'a str,
    config: Config,
    config_dir: PathBuf,
    chat: Option<ChatContext>,
    key: String,
    session_id: String,
    cwd: PathBuf,
    /// The checkout's top folder and work context, read from git once per hook run.
    top: Option<PathBuf>,
    context: OnceCell<WorkContext>,
    /// The brief reply's `check`: what the server wants looked up on the default branch.
    check: OnceCell<Value>,
    payload: Payload,
}

impl Hook<'_> {
    /// The text to inject, if the event injects any.
    fn handle(&self) -> Result<Option<String>, String> {
        if !matches!(self.event, "PreToolUse" | "PostToolUseFailure") {
            self.note_automation();
        }
        match self.event {
            "PreToolUse" => Ok(self.command_recall(false)),
            "PostToolUseFailure" => Ok(self.command_recall(true)),
            "SessionStart" => Ok(self.start()),
            "UserPromptSubmit" | "PreInvocation" => {
                let prompt = self.payload.prompt();
                if let Some(capture) = &prompt {
                    if has_typed_line(&capture.content) {
                        self.with_state(|s| s.worked_signal = true);
                    }
                    self.note_correction(&capture.content);
                }
                let capture_result = self.capture(prompt);
                let mut recall = self
                    .payload
                    .prompt_text()
                    .and_then(|text| self.prompt_recall(text));
                let mut pending = Vec::new();
                if self.agent == "grok" && recall.is_some() {
                    self.with_state(|s| s.pending_prompt_recall = recall.take());
                }
                if self.event == "PreInvocation" {
                    pending = self
                        .with_state(|s| std::mem::take(&mut s.pending_command_recall))
                        .unwrap_or_default();
                }
                pending.extend(recall);
                if pending.is_empty() {
                    capture_result?;
                    Ok(None)
                } else {
                    if let Err(error) = capture_result {
                        eprintln!("cairn hook: {error}");
                    }
                    Ok(Some(pending.join("\n")))
                }
            }
            "PostToolUse" => {
                let brief = (self.agent == "grok"
                    && self
                        .with_state(|s| !std::mem::replace(&mut s.grok_brief_shown, true))
                        .unwrap_or(false))
                .then(|| self.brief())
                .flatten();
                let topic = self.tool()?;
                let prompt = (self.agent == "grok")
                    .then(|| {
                        self.with_state(|s| s.pending_prompt_recall.take())
                            .flatten()
                    })
                    .flatten();
                let warning = (self.agent == "hermes")
                    .then(|| self.command_recall(false))
                    .flatten();
                let mut recall = self.command_recall(true);
                if self.agent == "antigravity"
                    && let Some(text) = recall.take()
                {
                    self.with_state(|s| s.pending_command_recall.push(text));
                }
                let text: Vec<_> = [brief, prompt, topic, warning, recall]
                    .into_iter()
                    .flatten()
                    .collect();
                Ok((!text.is_empty()).then(|| text.join("\n")))
            }
            "Stop" | "SubagentStop" => {
                if self.agent == "antigravity" && self.payload.flag("fullyIdle") {
                    self.with_state(|s| s.last_prompt_recall = None);
                }
                if std::env::var("CAIRN_CAPTURE_ASSISTANT").as_deref() == Ok("1") {
                    self.capture(self.payload.assistant())?;
                }
                if let Some(generation) = self
                    .with_state(|s| s.dirty.then_some(s.generation))
                    .flatten()
                {
                    self.capture_changed_files();
                    self.checkpoint(generation);
                }
                // SubagentStop captures like Stop and never asks.
                if self.event == "SubagentStop" {
                    Ok(None)
                } else {
                    Ok(self.stop_nudge())
                }
            }
            "PreCompact" => {
                if let Some(generation) = self
                    .with_state(|s| s.worked.then_some(s.generation))
                    .flatten()
                {
                    self.capture_changed_files();
                    self.checkpoint(generation);
                }
                Ok(None)
            }
            "SessionEnd" => Ok(self.end()),
            _ => Ok(None),
        }
    }

    fn chat_tool(&self) -> Result<Value, Error> {
        let arguments = self
            .payload
            .value("arguments")
            .and_then(Value::as_object)
            .ok_or_else(|| Error::Input("chat tool arguments must be an object".into()))?;
        if self.event == "ChatSkill" {
            let tool = self
                .payload
                .value("skill_tool")
                .and_then(Value::as_str)
                .and_then(|name| {
                    crate::tools::SKILL_TOOLS
                        .iter()
                        .find(|tool| tool.name == name)
                })
                .ok_or_else(|| Error::Input("unknown chat skill tool".into()))?;
            let schema = (tool.schema)();
            let body: serde_json::Map<_, _> = arguments
                .iter()
                .filter(|(key, _)| schema["properties"].get(key.as_str()).is_some())
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            return self.post(
                &format!("/cairn-memory{}", tool.route),
                &Value::Object(body),
            );
        }
        let (route, keys): (&str, &[&str]) = if self.event == "ChatSearch" {
            ("/cairn-memory/search", &["query"])
        } else {
            (
                "/cairn-memory/remember",
                &["kind", "title", "summary", "reason", "fields"],
            )
        };
        let mut body = serde_json::Map::new();
        for key in keys {
            if let Some(value) = arguments.get(*key) {
                body.insert((*key).to_owned(), value.clone());
            }
        }
        let mut work = self.work_context();
        if let Some(project) = arguments.get("project") {
            work.project_id = Some(
                project
                    .as_str()
                    .ok_or_else(|| {
                        Error::Input("project must be a project ID or exact name".into())
                    })?
                    .to_owned(),
            );
        }
        body.insert("context".into(), json!(work));
        self.post(route, &Value::Object(body))
    }

    /// Asked on every chat hook call, so a server rollback turns chat memory off at once.
    fn chat_scope_supported(&self) -> bool {
        !self.server_is_down()
            && Client::identified(
                self.config.clone(),
                RECALL_TIMEOUT,
                self.agent,
                ClientKind::Hook,
            )
            .get("/cairn-memory/health")
            .ok()
            .and_then(|health| health["version"].as_str().and_then(update::parse_version))
            .is_some_and(|version| version >= CHAT_SCOPE_SERVER)
    }

    fn work_context(&self) -> WorkContext {
        if let Some(chat) = &self.chat {
            return WorkContext {
                chat: Some(chat.clone()),
                ..WorkContext::default()
            };
        }
        self.context
            .get_or_init(|| git::work_context(&self.cwd, self.top.as_deref()))
            .clone()
    }

    /// `<config_dir>/server-down.json`: `{"origin", "until"}` while the server is not asked.
    fn down_file(&self) -> PathBuf {
        self.config_dir.join("server-down.json")
    }

    fn server_is_down(&self) -> bool {
        let down: Value = std::fs::read_to_string(self.down_file())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        down["origin"] == self.config.url.as_str() && down["until"].as_i64() > Some(now_seconds())
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value, Error> {
        if self.server_is_down() {
            return Err(Error::Request(format!(
                "server did not answer; not asked again for {DOWN_SECONDS} s"
            )));
        }
        let result = Client::identified(
            self.config.clone(),
            REQUEST_TIMEOUT,
            self.agent,
            ClientKind::Hook,
        )
        .post(path, body);
        if let Err(error) = &result
            && let Some(outbox) = outbox::Outbox::open(&self.config, &self.config_dir)
        {
            outbox.failure(error);
        }
        match &result {
            // Only "no answer" (timeout, refused, unreachable); a 5xx answers quickly.
            Err(Error::Request(_)) => {
                let down =
                    json!({"origin": self.config.url, "until": now_seconds() + DOWN_SECONDS});
                let _ = write_private(&self.down_file(), down.to_string().as_bytes());
            }
            Ok(_) => {
                let _ = std::fs::remove_file(self.down_file());
            }
            Err(_) => {}
        }
        result
    }

    fn state_dir(&self) -> PathBuf {
        self.config_dir.join("hook-state")
    }

    /// Runs `change` on the locked session state and saves it. A busy or unwritable state is
    /// one stderr line and `None`; the hook carries on without it.
    fn with_state<R>(&self, change: impl FnOnce(&mut SessionState) -> R) -> Option<R> {
        let result = Locked::open(&self.state_dir(), &self.key).and_then(|mut locked| {
            let result = change(&mut locked.state);
            locked.save().map(|()| result)
        });
        result
            .map_err(|message| eprintln!("cairn hook: {message}"))
            .ok()
    }

    /// A short stderr line for a failed call; a 401 already names the config file.
    fn warn(&self, what: &str, error: &Error) {
        match error {
            Error::Status { status: 401, .. } => eprintln!("cairn hook: {error}"),
            _ => eprintln!("cairn hook: {what} ({error})"),
        }
    }

    fn start(&self) -> Option<String> {
        state::sweep(&self.state_dir());
        let dirty = if self.chat.is_some() {
            Vec::new()
        } else {
            git::dirty_files(&self.cwd, BASELINE_MAX)
        };
        self.with_state(|state| {
            state.baseline_dirty = dirty;
            state.grok_brief_shown = false;
        });
        (self.agent != "grok").then(|| self.brief()).flatten()
    }

    fn brief(&self) -> Option<String> {
        let body = json!({
            "session_id": self.session_id,
            "context": self.work_context(),
            "recent_files": if self.chat.is_some() { Vec::new() } else { git::recent_files(&self.cwd) },
            "extended_brief": true,
        });
        let reply = match self.post("/cairn-memory/session/brief", &body) {
            Ok(reply) => reply,
            Err(error) => {
                self.warn(
                    "Cairn brief unavailable; use memory tools when connected",
                    &error,
                );
                if self.chat.is_some() {
                    return Some(DEFAULT_CHAT_INSTRUCTION.to_owned());
                }
                return self
                    .config
                    .nudge()
                    .then_some(DEFAULT_SESSION_INSTRUCTION)
                    .map(str::to_owned);
            }
        };
        if let Some(check) = reply.get("check").filter(|check| check.is_object()) {
            let _ = self.check.set(check.clone());
        }
        let brief = reply
            .get("brief")
            .and_then(Value::as_str)
            .filter(|brief| !brief.is_empty())
            .filter(|brief| {
                let fits = brief.len() <= inject::BRIEF_MAX_BYTES;
                if !fits {
                    eprintln!(
                        "cairn hook: the brief exceeds {} bytes; not injected",
                        inject::BRIEF_MAX_BYTES
                    );
                }
                fits
            });
        let resolved = reply
            .get("project_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        if let Some(id) = resolved.clone() {
            self.with_state(|s| s.project_id = Some(id));
        }
        let line = if self.chat.is_some() {
            Some(inject::server_text(
                &reply,
                "chat_instruction",
                DEFAULT_CHAT_INSTRUCTION,
            ))
        } else if !self.config.nudge() {
            None
        } else {
            let stop = inject::server_text(&reply, "stop_nudge", DEFAULT_STOP_NUDGE);
            self.with_state(|s| s.stop_nudge = Some(stop));
            let mut instruction =
                inject::server_text(&reply, "session_instruction", DEFAULT_SESSION_INSTRUCTION);
            if resolved.is_none() && !self.file_project() {
                let first = self
                    .with_state(|state| {
                        let first = !state.hinted;
                        state.hinted = true;
                        first
                    })
                    .unwrap_or(false);
                if first {
                    instruction.push('\n');
                    instruction.push_str(NO_PROJECT_HINT);
                }
            }
            Some(instruction)
        };
        let notice = self.top.as_deref()
            .and_then(|top| crate::project::read_at(top).ok().flatten())
            .and_then(|file| {
                let server_name = reply.get("project_name")?.as_str()?;
                if resolved.as_deref() != Some(file.project_id.as_str()) || file.name.as_deref().is_none_or(|name| name == server_name) {
                    return None;
                }
                let first = self.with_state(|state| {
                    let first = !state.rename_noticed;
                    state.rename_noticed = true;
                    first
                }).unwrap_or(false);
                if !first { return None; }
                // Leave room for the instruction while keeping the notice short.
                let mut name: String = server_name.chars().filter(|c| !c.is_control()).scan(0, |len, c| {
                    *len += c.len_utf8();
                    (*len <= 24).then_some(c)
                }).collect();
                if name.len() < server_name.len() { name.push('…'); }
                Some(format!("Cairn: this project was renamed to {name:?}; run `cairn init` and commit .cairn/project.json."))
            });
        let line = match (line, notice) {
            (Some(line), Some(notice)) => Some(format!("{line}\n{notice}")),
            (None, Some(notice)) => Some(notice),
            (line, None) => line,
        };
        match (brief, line) {
            (Some(brief), Some(line)) => Some(format!("{brief}\n{line}")),
            (Some(brief), None) => Some(brief.to_owned()),
            (None, line) => line,
        }
    }

    /// A readable `.cairn/project.json` at the resolved root.
    fn file_project(&self) -> bool {
        self.top
            .as_deref()
            .is_some_and(|top| crate::project::project_id_at(top).is_some())
    }

    /// Reminders are on: not opted out, and a project was resolved from the file or the brief.
    fn nudging(&self) -> bool {
        self.config.nudge()
            && (self.file_project()
                || self
                    .with_state(|state| state.project_id.is_some())
                    .unwrap_or(false))
    }

    /// `CAIRN_SESSION_ORIGIN=automation` marks the whole session. A subagent payload does not:
    /// Claude Code sends those events on the parent session id.
    fn env_automated() -> bool {
        std::env::var("CAIRN_SESSION_ORIGIN").ok().as_deref() == Some("automation")
    }

    fn note_automation(&self) {
        if Self::env_automated() {
            self.with_state(|state| state.automated = true);
        }
    }

    /// Stored only from the env, so a later event still counts after `SessionStart`.
    fn session_automated(&self) -> bool {
        Self::env_automated() || self.with_state(|state| state.automated).unwrap_or(false)
    }

    /// Suppress the ask for this event: env, state stored from the env, or this event's payload.
    fn automated(&self) -> bool {
        self.session_automated()
            || self.payload.text("agent_id").is_some()
            || self.payload.text("agent_type").is_some()
            || self.payload.text("subagentType").is_some()
            || self.event == "SubagentStop"
    }

    /// Keeps the first correction line. An automated or subagent session records none.
    fn note_correction(&self, content: &str) {
        if self.automated() {
            return;
        }
        let Some(line) = first_correction(content) else {
            return;
        };
        self.with_state(|state| {
            if state.correction.is_none() {
                state.correction = Some(event::clip(&line, 95));
            }
        });
    }

    /// The one-time Stop reminder: the session edited files or the user corrected the agent, and
    /// never saved. A test command or a `Decision:` line alone does not ask. Not when the host is
    /// already continuing because of a Stop hook (`stop_hook_active`), not for an automated or
    /// subagent session, not for OpenCode, and not when the stop text is `off`.
    fn stop_nudge(&self) -> Option<String> {
        if self.payload.flag("stop_hook_active")
            || self.payload.flag("stopHookActive")
            || (self.agent == "grok"
                && matches!(
                    self.payload.text("reason"),
                    Some("channel_closed" | "shutdown")
                ))
            || (self.agent == "antigravity" && !self.payload.flag("fullyIdle"))
            || self.automated()
            || matches!(self.agent, "opencode" | "hermes")
            || !self.nudging()
        {
            return None;
        }
        let decided = self
            .payload
            .assistant()
            .is_some_and(|message| has_typed_line(&message.content));
        self.with_state(|s| {
            s.worked_signal |= decided;
            let template = s.stop_nudge.as_deref().unwrap_or(DEFAULT_STOP_NUDGE);
            let correction = s.correction.as_deref().filter(|line| !line.is_empty());
            if s.nudged
                || s.saved
                || template.is_empty()
                || (s.edited.is_empty() && correction.is_none())
            {
                return None;
            }
            s.nudged = true;
            let files = (!s.edited.is_empty()).then(|| fill_stop_nudge(template, s.edited.len()));
            let clause = correction.map(fill_correction_nudge);
            match (files, clause) {
                (Some(files), Some(clause)) => Some(format!("{files} {clause}")),
                (Some(files), None) => Some(files),
                (None, Some(clause)) => Some(clause),
                (None, None) => None,
            }
        })
        .flatten()
    }

    /// After the brief is printed: answers its `check` with what git says (files and symbols still
    /// there, branches merged). Two seconds, no outbox: a missed report is made on the next start.
    fn report_repo_state(&self) {
        let (Some(check), Some(top)) = (self.check.get(), self.top.as_deref()) else {
            return;
        };
        let list = |key: &str, max: usize| -> Vec<String> {
            check[key]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .take(max)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        let (files, symbols) = (list("files", 200), list("symbols", 20));
        let Some(facts) = git::repo_facts(top, &files, &symbols) else {
            return;
        };
        let body = json!({
            "project_id": check["project_id"],
            "branch": check["branch"],
            "commit": facts.commit,
            "files_missing": facts.files_missing,
            "files_present": facts.files_present,
            "symbols_missing": facts.symbols_missing,
            "symbols_present": facts.symbols_present,
            "merged_branches": facts.merged_branches,
        });
        if let Err(error) = self.post("/cairn-memory/session/repo-state", &body) {
            self.warn("Cairn repo state unavailable", &error);
        }
    }

    fn tool(&self) -> Result<Option<String>, String> {
        let Some(tool) = self.payload.tool() else {
            return Ok(None);
        };
        if matches!(self.agent, "antigravity" | "hermes")
            && tool.is_file_tool()
            && tool.ok() == Some(false)
        {
            return Ok(None);
        }
        if let Some(mut lookup) = tool.lookup() {
            lookup["session_id"] = json!(self.session_id);
            let _ = self.post("/cairn-memory/session/lookup", &lookup);
            return Ok(None);
        }
        if matches!(
            tool.name.as_str(),
            "mcp__cairn__memory_save" | "cairn_memory_save"
        ) {
            if tool.mcp_save_succeeded() {
                self.with_state(|s| s.saved = true);
            }
            return Ok(None);
        }
        // A read is neither captured nor work, but it shares the edit's once-per-path topic note
        // and per-session caps.
        if let Some(given) = tool.read_path() {
            let path = git::repo_relative(given, &self.cwd, self.top.as_deref());
            return Ok(path.and_then(|path| {
                let symbol = tool
                    .input_text("symbol")
                    .filter(|symbol| symbol.len() <= 200);
                let text = symbol.map(redact_sensitive_content).unwrap_or_default();
                self.topic(&path, &text)
            }));
        }
        if tool.is_read_only() {
            return Ok(None);
        }
        if tool.is_work() {
            let command = tool.shell_command();
            let tested = command.is_some_and(is_test_command);
            let saved = command.is_some_and(is_cairn_save) && tool.ok() == Some(true);
            self.with_state(|s| {
                s.dirty = true;
                s.worked = true;
                s.generation += 1;
                s.worked_signal |= tested;
                s.saved |= saved;
            });
        }
        if tool.is_file_tool() {
            let mut notes = Vec::new();
            for (given, new_text) in tool.edits() {
                let Some(path) = git::repo_relative(&given, &self.cwd, self.top.as_deref()) else {
                    if git::outside_project(&given, &self.cwd, self.top.as_deref()) {
                        self.with_state(|s| {
                            s.outside_edits = s.outside_edits.saturating_add(1);
                        });
                    }
                    continue;
                };
                self.with_state(|s| s.remember_edit(&path));
                let capture = tool.edit(&path, &new_text);
                let text = capture
                    .content
                    .split_once('\n')
                    .map_or("", |(_, text)| text)
                    .to_owned();
                self.capture(Some(capture))?;
                notes.extend(self.topic(&path, &text));
            }
            return Ok((!notes.is_empty()).then(|| notes.join("\n")));
        }
        self.capture(tool.command()).map(|()| None)
    }

    fn prompt_recall(&self, text: &str) -> Option<String> {
        if !should_recall(text) || self.server_is_down() {
            return None;
        }
        let prompt = redact_sensitive_content(text);
        let prompt = event::truncate(&prompt, PROMPT_QUERY_MAX_BYTES);
        let wanted = self.with_state(|s| {
            if s.prompt_budget_spent {
                return false;
            }
            if self.event == "PreInvocation" {
                let key = sha256_hex(&[prompt]);
                if s.last_prompt_recall.as_ref() == Some(&key) {
                    return false;
                }
                s.last_prompt_recall = Some(key);
            }
            true
        })?;
        if !wanted {
            return None;
        }
        let mut context = self.work_context();
        // Only this project's identity is needed; prompt text never goes through capture here.
        context = WorkContext {
            project_id: context.project_id.take(),
            branch: context.branch.take(),
            chat: context.chat.take(),
            ..WorkContext::default()
        };
        let body = json!({"prompt": prompt, "session_id": self.session_id, "context": context});
        let reply = Client::identified(
            self.config.clone(),
            RECALL_TIMEOUT,
            self.agent,
            ClientKind::Hook,
        )
        .post("/cairn-memory/session/prompt-recall", &body)
        .ok()?;
        if reply["budget_spent"] == true {
            self.with_state(|s| s.prompt_budget_spent = true);
        }
        let text = reply["text"].as_str()?;
        (!text.is_empty() && text.len() <= inject::TOPIC_MAX_BYTES).then(|| text.to_owned())
    }

    /// This lookup has a short independent deadline and never queues observations or logs.
    fn command_recall(&self, failed: bool) -> Option<String> {
        let tool = self.payload.tool()?;
        let command = tool.shell_command()?;
        let error_tail = if failed {
            Some(error_tail(
                tool.failure_text(self.event == "PostToolUseFailure")?,
            ))
        } else {
            None
        };
        let command = normalize_command(&redact_sensitive_content(command));
        if command.is_empty() || command.chars().count() > 200 || self.server_is_down() {
            return None;
        }
        let project_id = if self.chat.is_some() {
            self.payload.text("project_id").map(str::to_owned)
        } else {
            self.top.as_deref().and_then(git::project_id)
        };
        let mut body = json!({
            "command": command,
            "session_id": self.session_id,
            "project_id": project_id,
            "error_tail": error_tail,
        });
        if let Some(chat) = &self.chat {
            body["chat"] = json!(chat);
        }
        let reply = Client::identified(
            self.config.clone(),
            RECALL_TIMEOUT,
            self.agent,
            ClientKind::Hook,
        )
        .post("/cairn-memory/session/command-recall", &body)
        .ok()?;
        let text = reply["text"].as_str()?;
        if failed && !text.is_empty() {
            self.with_state(|s| s.command_error_recalled = true);
        }
        (!text.is_empty() && text.len() <= inject::TOPIC_MAX_BYTES).then(|| text.to_owned())
    }

    /// The note text for the first edit of `path`, within the per-session caps.
    fn topic(&self, path: &str, tool_text: &str) -> Option<String> {
        if matches!(self.agent, "antigravity" | "hermes") {
            return None; // The host discards context returned from PostToolUse.
        }
        // The lookup is reserved before the request, so a failing server is not asked without end.
        let wanted = self.with_state(|s| {
            let wanted = !s.topics.iter().any(|seen| seen == path)
                && s.topics_injected < inject::TOPICS_PER_SESSION
                && s.topic_lookups < inject::TOPIC_LOOKUPS_PER_SESSION;
            if wanted {
                s.topic_lookups += 1;
                s.topics_injected += 1;
                s.remember_topic(path);
            }
            wanted
        })?;
        if !wanted {
            return None;
        }
        let mut context = self.work_context();
        for token in tool_text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != ':') {
            let token = token.trim_matches(':');
            if token.len() <= 200
                && is_symbol(token)
                && !context.symbols.iter().any(|symbol| symbol == token)
            {
                context.symbols.push(token.to_owned());
                if context.symbols.len() == ENTITY_CAP {
                    break;
                }
            }
        }
        let body = json!({"path": path, "session_id": self.session_id, "context": context});
        let reply = match self.post("/cairn-memory/session/topic", &body) {
            Ok(reply) => reply,
            Err(error) => {
                self.with_state(|s| {
                    s.topics.retain(|seen| seen != path);
                    s.topics_injected = s.topics_injected.saturating_sub(1);
                });
                self.warn("Cairn topic notes unavailable", &error);
                return None;
            }
        };
        let notes: Vec<(&str, &str, &str, &str)> = reply["notes"]
            .as_array()
            .map(|notes| {
                notes
                    .iter()
                    .filter_map(|note| {
                        let text = |key: &str| note[key].as_str().unwrap_or_default();
                        Some((text("title"), text("summary"), text("why"), text("age")))
                            .filter(|(title, ..)| !title.is_empty())
                    })
                    .collect()
            })
            .unwrap_or_default();
        let text = match reply["card"].as_str().filter(|card| !card.is_empty()) {
            Some(card) => inject::card(card),
            None => inject::topic(path, &notes).0,
        };
        if text.is_empty() {
            self.with_state(|s| s.topics_injected = s.topics_injected.saturating_sub(1));
            return None;
        }
        Some(text)
    }

    /// Sends one typed capture unless this session already sent the same content. A transient
    /// failure queues the same body for replay; a rejection is reported and dropped. The repeat
    /// check remembers a capture only once it is sent or queued, so one that never left is sent
    /// again after the cause (a bad token, a busy outbox) is fixed.
    fn capture(&self, capture: Option<Capture>) -> Result<(), String> {
        self.capture_outside(capture, 0)
    }

    /// Paths dirty since SessionStart, plus tool edits, capped. Skipped when that set is empty
    /// or the same set was already sent. `outside_edits` rides along as a count.
    fn capture_changed_files(&self) {
        if self.chat.is_some() {
            return;
        }
        let cwd = self.cwd.clone();
        let Some(Some((files, outside, hash))) = self.with_state(|state| {
            let files = changed_paths(state, &cwd);
            if files.is_empty() {
                return None;
            }
            let hash = sha256_hex(&files.iter().map(String::as_str).collect::<Vec<_>>());
            if state.files_sent.as_deref() == Some(hash.as_str()) {
                return None;
            }
            Some((files, state.outside_edits, hash))
        }) else {
            return;
        };
        let capture = Capture {
            kind: CaptureKind::Edit,
            ok: None,
            content: format!("changed files\n{}", files.join("\n")),
            files,
            commands: Vec::new(),
        };
        if self.capture_outside(Some(capture), outside).is_ok() {
            self.with_state(|state| state.files_sent = Some(hash));
        }
    }

    fn capture_outside(&self, capture: Option<Capture>, outside_files: u32) -> Result<(), String> {
        let Some(capture) = capture else {
            return Ok(());
        };
        let (event_id, hash) = if capture.kind == CaptureKind::Command {
            // A repeat execution has a new tool id. The outbox retains the id assigned here.
            let event_id = self.payload.tool_use_id().map_or_else(
                || new_id("hk"),
                |id| {
                    format!(
                        "hk_{}",
                        &sha256_hex(&[&self.session_id, "command", id])[..24]
                    )
                },
            );
            let hash = sha256_hex(&[&event_id, &capture.content_hash()])[..16].to_owned();
            (event_id, hash)
        } else {
            (capture.event_id(&self.session_id), capture.content_hash())
        };
        if self.with_state(|s| s.capture_hashes.contains(&hash)) == Some(true) {
            return Ok(());
        }
        let remember = || {
            self.with_state(|s| s.remember_capture(&hash));
        };
        let mut context = self.work_context();
        (context.files, context.commands) = (capture.files, capture.commands);
        context.outside_files = outside_files;
        // Origin sticks in the jsonb merge, so only an env-marked session may set it.
        if self.session_automated() {
            context.origin = Some("automation".to_owned());
        }
        let mut body = json!({
            "event_id": event_id,
            "session_id": self.session_id,
            "kind": capture.kind.as_str(),
            "content": capture.content,
            "context": redact_work_context(&context),
        });
        if let Some(ok) = capture.ok {
            body["ok"] = json!(ok);
        }
        let outbox = outbox::Outbox::open(&self.config, &self.config_dir);
        let (depth, health) = outbox.as_ref().and_then(|o| o.health()).unwrap_or_default();
        body["outbox_depth"] = json!(depth);
        if let Some(error) = &health {
            body["hook_error_class"] = json!(error.class);
            body["hook_error_at"] = json!(error.at);
        }
        let error = match self.post(OBSERVE, &body) {
            Ok(_) => {
                if let Some(outbox) = &outbox {
                    outbox.clear_error(&health);
                }
                remember();
                if let Some(outbox) = outbox::Outbox::open(&self.config, &self.config_dir) {
                    outbox.replay(&self.config, self.agent, &event_id, self.chat.is_some());
                }
                return Ok(());
            }
            Err(error) => error,
        };
        if !outbox::retryable(&error) {
            return Err(match error {
                Error::Status { status: 401, .. } => error.to_string(),
                _ => format!("Cairn rejected the capture ({error})"),
            });
        }
        let outbox = outbox::Outbox::open(&self.config, &self.config_dir)
            .ok_or("outbox is bound to another server or device; the capture was not retained")?;
        let dropped = outbox.enqueue(&event_id, &body, &error)?;
        remember();
        if dropped > 0 {
            eprintln!("cairn hook: outbox full; dropped {dropped} oldest capture(s)");
        }
        eprintln!("cairn hook: capture queued for replay ({error})");
        Ok(())
    }

    /// Keeps the session's work: on a stored result `dirty` clears unless more work arrived
    /// while the request ran.
    fn checkpoint(&self, generation: u64) {
        let mut body = json!({"session_id": self.session_id});
        if let Some(chat) = &self.chat {
            body["chat"] = json!(chat);
        }
        match self.post("/cairn-memory/session/checkpoint", &body) {
            Ok(reply)
                if matches!(
                    reply["status"].as_str(),
                    Some("stored" | "unchanged" | "empty")
                ) =>
            {
                self.with_state(|s| {
                    if s.generation == generation {
                        s.dirty = false;
                    }
                });
            }
            Ok(_) => eprintln!(
                "cairn hook: Cairn checkpoint unavailable; it will retry after the next meaningful turn"
            ),
            Err(error) => self.warn(
                "Cairn checkpoint unavailable; it will retry after the next meaningful turn",
                &error,
            ),
        }
    }

    fn end(&self) -> Option<String> {
        let locked = Locked::open(&self.state_dir(), &self.key)
            .map_err(|message| eprintln!("cairn hook: {message}"))
            .ok()?;
        let worked = locked.state.worked;
        let used_recall = locked.state.command_error_recalled;
        let nudged = locked.state.nudged;
        let saved = locked.state.saved;
        let automated = locked.state.automated
            || Self::env_automated()
            || self.payload.text("agent_id").is_some()
            || self.payload.text("agent_type").is_some()
            || self.payload.text("subagentType").is_some();
        drop(locked);
        if worked {
            self.capture_changed_files();
        }
        if let Ok(locked) = Locked::open(&self.state_dir(), &self.key) {
            locked.delete();
        }
        if worked || used_recall || self.chat.is_some() {
            let mut body = json!({"session_id": self.session_id});
            if let Some(chat) = &self.chat {
                body["chat"] = json!(chat);
            } else if !automated {
                body["nudge_asked"] = json!(nudged);
                body["nudge_saved"] = json!(saved);
            }
            if let Err(error) = self.post("/cairn-memory/session/end", &body) {
                self.warn(
                    "Cairn session close unavailable; idle distillation remains pending",
                    &error,
                );
            }
        }
        None
    }
}
