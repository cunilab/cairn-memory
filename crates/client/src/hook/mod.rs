//! `cairn hook <Event> [--agent a] [--config p] [--cairn-managed]`: the capture and injection
//! hook of every agent host (docs/reference/hook.md#events). The host's event arrives as JSON on stdin; stdout
//! carries only an injection reply. A failure writes one stderr line and still exits 0, so a
//! hook can never break the host.

mod event;
mod git;
mod inject;
mod outbox;
mod readonly;
mod state;

use crate::{Client, Config, Error, selected_config_path, write_private};
use cairn_core::account::ClientKind;
use cairn_core::capture::{CaptureKind, WorkContext, redact_work_context};
use cairn_core::id::{new_id, now_seconds, sha256_hex};
use cairn_core::instructions::{
    DEFAULT_SESSION_INSTRUCTION, DEFAULT_STOP_NUDGE, fill_correction_nudge, fill_stop_nudge,
};
use cairn_core::process::distill::{
    HANDOFF_FILES_MAX, first_correction, has_typed_line, is_test_command,
};
use event::{Capture, Payload};
use serde_json::{Value, json};
use state::{Locked, SessionState};
use std::cell::OnceCell;
use std::path::PathBuf;
use std::time::Duration;

const OBSERVE: &str = "/cairn-memory/observe";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
/// After a request got no answer, further requests are skipped (captures go straight to the
/// outbox) for this long, so a dead network costs one timeout instead of one per hook call.
const DOWN_SECONDS: i64 = 30;
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
    if let Err(message) = session(&options) {
        eprintln!("cairn hook: {message}");
    }
    Ok(0)
}

fn session(options: &Options) -> Result<(), String> {
    crate::warn_ignored_legacy_config();
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
    let raw = std::io::read_to_string(std::io::stdin()).map_err(|_| "cannot read hook input")?;
    let payload = Payload::parse(&raw)?;
    let cwd = match payload.cwd() {
        Some(cwd) => PathBuf::from(cwd),
        None => std::env::current_dir().map_err(|e| format!("no working folder: {e}"))?,
    };
    let top = git::top_level(&cwd);
    if git::ignored(&cwd, top.as_deref()) {
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
        key,
        session_id,
        cwd,
        top,
        context: OnceCell::new(),
        check: OnceCell::new(),
        payload,
    };
    if let Some(text) = hook.handle()? {
        println!("{}", inject::output(hook.event, &text));
    }
    hook.report_repo_state();
    Ok(())
}

struct Hook<'a> {
    event: &'a str,
    /// `claude`, `codex` or `opencode`: sent in the user agent so the console knows the device.
    agent: &'a str,
    config: Config,
    config_dir: PathBuf,
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
        self.note_automation();
        match self.event {
            "SessionStart" => Ok(self.start()),
            "UserPromptSubmit" => {
                let prompt = self.payload.prompt();
                if let Some(capture) = &prompt {
                    if has_typed_line(&capture.content) {
                        self.with_state(|s| s.worked_signal = true);
                    }
                    self.note_correction(&capture.content);
                }
                self.capture(prompt).map(|()| None)
            }
            "PostToolUse" => self.tool(),
            "Stop" | "SubagentStop" => {
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

    fn work_context(&self) -> WorkContext {
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
        let dirty = git::dirty_files(&self.cwd, BASELINE_MAX);
        self.with_state(|state| state.baseline_dirty = dirty);
        let body = json!({
            "session_id": self.session_id,
            "context": self.work_context(),
            "recent_files": git::recent_files(&self.cwd),
        });
        let reply = match self.post("/cairn-memory/session/brief", &body) {
            Ok(reply) => reply,
            Err(error) => {
                self.warn(
                    "Cairn brief unavailable; use memory tools when connected",
                    &error,
                );
                return self
                    .config
                    .nudge()
                    .then(|| DEFAULT_SESSION_INSTRUCTION.to_owned());
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
        let line = if !self.config.nudge() {
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
            || self.automated()
            || self.agent == "opencode"
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
            return Ok(path.and_then(|path| self.topic(&path)));
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
                self.capture(Some(tool.edit(&path, &new_text)))?;
                notes.extend(self.topic(&path));
            }
            return Ok((!notes.is_empty()).then(|| notes.join("\n")));
        }
        self.capture(tool.command()).map(|()| None)
    }

    /// The note text for the first edit of `path`, within the per-session caps.
    fn topic(&self, path: &str) -> Option<String> {
        // The lookup is reserved before the request, so a failing server is not asked without end.
        let wanted = self.with_state(|s| {
            let wanted = !s.topics.iter().any(|seen| seen == path)
                && s.topics_injected < inject::TOPICS_PER_SESSION
                && s.topic_lookups < inject::TOPIC_LOOKUPS_PER_SESSION;
            s.topic_lookups += u8::from(wanted);
            wanted
        })?;
        if !wanted {
            return None;
        }
        let body = json!({"path": path, "context": self.work_context()});
        let reply = match self.post("/cairn-memory/session/topic", &body) {
            Ok(reply) => reply,
            // Not marked seen: the next edit of this path asks again, while lookups remain.
            Err(error) => {
                self.warn("Cairn topic notes unavailable", &error);
                return None;
            }
        };
        let notes: Vec<(String, String)> = reply["notes"]
            .as_array()
            .map(|notes| {
                notes
                    .iter()
                    .filter_map(|note| {
                        let text = |key: &str| note[key].as_str().unwrap_or_default().to_owned();
                        Some((text("title"), text("summary")))
                            .filter(|(title, _)| !title.is_empty())
                    })
                    .collect()
            })
            .unwrap_or_default();
        let text = match reply["card"].as_str().filter(|card| !card.is_empty()) {
            Some(card) => inject::card(card),
            None => inject::topic(path, &notes),
        };
        let keep = self.with_state(|s| {
            // A parallel hook of this session may have answered first: one note per path.
            if s.topics.iter().any(|seen| seen == path) {
                return false;
            }
            s.remember_topic(path);
            let fits = !text.is_empty() && s.topics_injected < inject::TOPICS_PER_SESSION;
            if fits {
                s.topics_injected += 1;
            }
            fits
        });
        keep.unwrap_or(false).then_some(text)
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
            let event_id = self.payload.text("tool_use_id").map_or_else(
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
                    outbox.replay(&self.config, self.agent, &event_id);
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
        let body = json!({"session_id": self.session_id});
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
        let nudged = locked.state.nudged;
        let saved = locked.state.saved;
        let automated = locked.state.automated
            || Self::env_automated()
            || self.payload.text("agent_id").is_some()
            || self.payload.text("agent_type").is_some();
        drop(locked);
        if worked {
            self.capture_changed_files();
        }
        if let Ok(locked) = Locked::open(&self.state_dir(), &self.key) {
            locked.delete();
        }
        if worked {
            let mut body = json!({"session_id": self.session_id});
            if !automated {
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
