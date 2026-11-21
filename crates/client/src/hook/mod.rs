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

use crate::{Client, Config, Error, selected_config_path};
use cairn_core::capture::{WorkContext, redact_work_context};
use event::{Capture, Payload};
use serde_json::{Value, json};
use state::{Locked, SessionState};
use std::path::PathBuf;
use std::time::Duration;

const OBSERVE: &str = "/cairn-memory/observe";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

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
    if git::ignored(&cwd) {
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
        config,
        config_dir,
        key,
        session_id,
        cwd,
        payload,
    };
    if let Some(text) = hook.handle()? {
        println!("{}", inject::output(hook.event, &text));
    }
    Ok(())
}

struct Hook<'a> {
    event: &'a str,
    config: Config,
    config_dir: PathBuf,
    key: String,
    session_id: String,
    cwd: PathBuf,
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

    fn post(&self, path: &str, body: &Value) -> Result<Value, Error> {
        Client::with_timeout(self.config.clone(), REQUEST_TIMEOUT).post(path, body)
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
            "context": git::work_context(&self.cwd),
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
        // A new context (start, resume, compaction) starts its budget with the brief.
        self.with_state(|s| s.injected_bytes = brief.len() as u32);
        Some(brief.to_owned())
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
            let path = tool
                .file_path()
                .and_then(|path| git::repo_relative(path, &self.cwd));
            let Some(path) = path else { return Ok(None) };
            self.capture(Some(tool.edit(&path)))?;
            return Ok(self.topic(&path));
        }
        self.capture(tool.command()).map(|()| None)
    }

    /// The note text for the first edit of `path`, within the per-session caps.
    fn topic(&self, path: &str) -> Option<String> {
        let wanted = self.with_state(|s| {
            !s.topics.iter().any(|seen| seen == path)
                && s.topics_injected < inject::TOPICS_PER_SESSION
                && s.injected_bytes as usize + inject::TOPIC_MAX_BYTES <= inject::TOTAL_MAX_BYTES
        })?;
        if !wanted {
            return None;
        }
        let body = json!({"path": path, "context": git::work_context(&self.cwd)});
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
        let text = inject::topic(path, &notes);
        let keep = self.with_state(|s| {
            s.remember_topic(path);
            let fits = s.topics_injected < inject::TOPICS_PER_SESSION
                && s.injected_bytes as usize + text.len() <= inject::TOTAL_MAX_BYTES;
            if !text.is_empty() && fits {
                s.topics_injected += 1;
                s.injected_bytes += text.len() as u32;
            }
            !text.is_empty() && fits
        });
        keep.unwrap_or(false).then_some(text)
    }

    /// Sends one typed capture unless this session already sent the same content. A transient
    /// failure queues the same body for replay; a rejection is reported and dropped.
    fn capture(&self, capture: Option<Capture>) -> Result<(), String> {
        let Some(capture) = capture else {
            return Ok(());
        };
        if self.with_state(|s| s.remember_capture(&capture.content_hash())) == Some(false) {
            return Ok(());
        }
        let event_id = capture.event_id(&self.session_id, self.payload.tool_use_id());
        let mut context: WorkContext = git::work_context(&self.cwd);
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
                if let Some(outbox) = outbox::Outbox::open(&self.config, &self.config_dir) {
                    outbox.replay(&self.config, &event_id);
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
