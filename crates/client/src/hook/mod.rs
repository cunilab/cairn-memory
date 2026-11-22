//! `cairn hook <Event> [--agent a] [--config p] [--cairn-managed]`: the capture and injection
//! hook of every agent host (CM-SPEC-006 §1). The host's event arrives as JSON on stdin; stdout
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
use cairn_core::capture::{WorkContext, redact_work_context};
use cairn_core::id::now_seconds;
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
        match self.event {
            "SessionStart" => Ok(self.start()),
            "UserPromptSubmit" => self.capture(self.payload.prompt()).map(|()| None),
            "PostToolUse" => self.tool(),
            "Stop" => {
                if std::env::var("CAIRN_CAPTURE_ASSISTANT").as_deref() == Ok("1") {
                    self.capture(self.payload.assistant())?;
                }
                if let Some(generation) = self
                    .with_state(|s| s.dirty.then_some(s.generation))
                    .flatten()
                {
                    self.checkpoint(generation);
                }
                Ok(None)
            }
            "PreCompact" => {
                if let Some(generation) = self
                    .with_state(|s| s.worked.then_some(s.generation))
                    .flatten()
                {
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
                return None;
            }
        };
        if let Some(check) = reply.get("check").filter(|check| check.is_object()) {
            let _ = self.check.set(check.clone());
        }
        let brief = reply
            .get("brief")?
            .as_str()
            .filter(|brief| !brief.is_empty())?;
        if brief.len() > inject::BRIEF_MAX_BYTES {
            eprintln!(
                "cairn hook: the brief exceeds {} bytes; not injected",
                inject::BRIEF_MAX_BYTES
            );
            return None;
        }
        Some(brief.to_owned())
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
        let Some(tool) = self.payload.tool().filter(|tool| !tool.is_read_only()) else {
            return Ok(None);
        };
        if tool.is_work() {
            self.with_state(|s| {
                s.dirty = true;
                s.worked = true;
                s.generation += 1;
            });
        }
        if tool.is_file_tool() {
            let mut notes = Vec::new();
            for (given, new_text) in tool.edits() {
                let Some(path) = git::repo_relative(&given, &self.cwd, self.top.as_deref()) else {
                    continue;
                };
                self.capture(Some(tool.edit(&path, &new_text)))?;
                notes.extend(self.topic(&path));
            }
            return Ok((!notes.is_empty()).then(|| notes.join("\n")));
        }
        self.capture(tool.command()).map(|()| None)
    }

    /// The note text for the first edit of `path`, within the per-session caps.
    fn topic(&self, path: &str) -> Option<String> {
        let wanted = self.with_state(|s| {
            !s.topics.iter().any(|seen| seen == path)
                && s.topics_injected < inject::TOPICS_PER_SESSION
        })?;
        if !wanted {
            return None;
        }
        let body = json!({"path": path, "context": self.work_context()});
        let reply = match self.post("/cairn-memory/session/topic", &body) {
            Ok(reply) => reply,
            // Not marked seen: the next edit of this path asks again.
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
        let Some(capture) = capture else {
            return Ok(());
        };
        let hash = capture.content_hash();
        if self.with_state(|s| s.capture_hashes.contains(&hash)) == Some(true) {
            return Ok(());
        }
        let remember = || {
            self.with_state(|s| s.remember_capture(&hash));
        };
        let event_id = capture.event_id(&self.session_id);
        let mut context = self.work_context();
        (context.files, context.commands) = (capture.files, capture.commands);
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
        let error = match self.post(OBSERVE, &body) {
            Ok(_) => {
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
        locked.delete();
        if worked {
            let body = json!({"session_id": self.session_id});
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
