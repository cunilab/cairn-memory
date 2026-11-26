//! The offline outbox: captures that could not be delivered wait in
//! `<config_dir>/outbox.json`, bound to one origin and device, and are replayed after the next
//! successful POST (apps/docs/content/technical/hooks/index.md#outbox). The lock is held only to read or write the file, never
//! while a request is in flight, so a capture that fails during a replay is still kept.

use super::OBSERVE;
use crate::{Client, Config, Error, create_private_dir, write_private};
use cairn_core::account::ClientKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const VERSION: u32 = 3;
const CAP: usize = 256;
const MAX_ATTEMPTS: u32 = 10;
const REPLAY_LIMIT: usize = 20;
const REPLAY_BUDGET: Duration = Duration::from_secs(2);
const LOCK_WAIT: Duration = Duration::from_secs(1);
const BUSY: &str = "outbox is busy; the capture was not retained";

#[derive(Serialize, Deserialize)]
struct Queue {
    version: u32,
    origin: String,
    device: String,
    events: Vec<Pending>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookError {
    pub class: String,
    pub at: i64,
    #[serde(default)]
    nonce: String,
}

#[derive(Serialize, Deserialize)]
struct Pending {
    event_id: String,
    /// The redacted `/observe` body; never the token.
    body: Value,
    attempts: u32,
    last_error: Option<String>,
}

/// A transient failure worth queueing: no answer, 429 or 5xx.
pub fn retryable(error: &Error) -> bool {
    match error {
        Error::Request(_) => true,
        Error::Status { status, .. } => *status == 429 || *status >= 500,
        _ => false,
    }
}

pub struct Outbox {
    path: PathBuf,
    origin: String,
    device: String,
}

impl Outbox {
    /// `$CAIRN_OUTBOX` or `<config_dir>/outbox.json`; a file bound to another origin or device
    /// (or not an outbox) is left alone and `outbox.<device>.json` beside it is used instead.
    /// `None` when that one is foreign too.
    pub fn open(config: &Config, config_dir: &Path) -> Option<Self> {
        let digest = Sha256::digest(format!("cairn-outbox-v3\0{}", config.token));
        let device = format!("{digest:x}")[..32].to_owned();
        let main = match std::env::var_os("CAIRN_OUTBOX").filter(|value| !value.is_empty()) {
            Some(path) => PathBuf::from(path),
            None => config_dir.join("outbox.json"),
        };
        let alternative = main.with_file_name(format!("outbox.{device}.json"));
        [main, alternative]
            .into_iter()
            .map(|path| Self {
                path,
                origin: config.url.clone(),
                device: device.clone(),
            })
            .find(|outbox| outbox.read().is_some())
    }

    /// The queue on disk; an absent file is an empty queue; `None` when it is not ours.
    fn read(&self) -> Option<Queue> {
        let empty = || Queue {
            version: VERSION,
            origin: self.origin.clone(),
            device: self.device.clone(),
            events: Vec::new(),
        };
        let Ok(text) = fs::read_to_string(&self.path) else {
            return (!self.path.exists()).then(empty);
        };
        let mut queue: Queue = serde_json::from_str(&text).ok()?;
        if queue.version != VERSION || queue.origin != self.origin || queue.device != self.device {
            return None;
        }
        // An older gateway may have queued a capture before sender metadata was removed.
        for event in &mut queue.events {
            if let Some(chat) = event
                .body
                .pointer_mut("/context/chat")
                .and_then(Value::as_object_mut)
            {
                chat.remove("sender_id_hash");
                chat.remove("owner");
            }
        }
        Some(queue)
    }

    /// Waits up to `wait` for the lock; the holder only reads or writes one small file.
    fn lock(&self, wait: Duration) -> Result<File, String> {
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(".lock");
        let lock_path = self.path.with_file_name(name);
        if let Some(dir) = lock_path.parent() {
            create_private_dir(dir)
                .map_err(|error| format!("cannot create outbox directory: {error}"))?;
        }
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).write(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let lock = options
            .open(&lock_path)
            .map_err(|error| format!("cannot open outbox lock: {error}"))?;
        let started = Instant::now();
        loop {
            match lock.try_lock() {
                Ok(()) => return Ok(lock),
                Err(TryLockError::WouldBlock) if started.elapsed() < wait => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return Err(BUSY.to_owned()),
            }
        }
    }

    fn write(&self, queue: &Queue) -> Result<(), String> {
        let body = serde_json::to_vec(queue).map_err(|error| error.to_string())?;
        write_private(&self.path, &body).map_err(|error| format!("cannot write outbox: {error}"))
    }

    /// Appends one event; at 256 the oldest is dropped. `Ok(dropped)` counts those.
    pub fn enqueue(&self, event_id: &str, body: &Value, error: &Error) -> Result<usize, String> {
        let _lock = self.lock(LOCK_WAIT)?;
        let mut queue = self
            .read()
            .ok_or("outbox is bound to another server or device")?;
        if queue
            .events
            .iter()
            .any(|pending| pending.event_id == event_id)
        {
            return Ok(0);
        }
        queue.events.push(Pending {
            event_id: event_id.to_owned(),
            body: body.clone(),
            attempts: 0,
            last_error: Some(error.to_string().chars().take(200).collect()),
        });
        let dropped = queue.events.len().saturating_sub(CAP);
        queue.events.drain(..dropped);
        self.write(&queue)?;
        Ok(dropped)
    }

    fn health_path(&self) -> PathBuf {
        let digest = Sha256::digest(format!("{}\0{}", self.origin, self.device));
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(format!(".{digest:x}.health"));
        self.path.with_file_name(name)
    }

    pub fn health(&self) -> Option<(usize, Option<HookError>)> {
        let _lock = self.lock(LOCK_WAIT).ok()?;
        let queue = self.read()?;
        let error = fs::read(self.health_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        Some((queue.events.len(), error))
    }

    pub fn failure(&self, error: &Error) {
        let class = match error {
            Error::Status { status: 409, .. } => return,
            Error::Status {
                status: 401 | 403, ..
            } => "auth",
            Error::Status { status, .. } if *status >= 500 => "server",
            Error::Request(message)
                if message.contains("invalid JSON") || message.contains("cannot read response") =>
            {
                "parse"
            }
            Error::Request(_) => "network",
            _ => "parse",
        };
        let Ok(_lock) = self.lock(LOCK_WAIT) else {
            return;
        };
        let error = HookError {
            class: class.into(),
            at: cairn_core::id::now_seconds(),
            nonce: cairn_core::id::new_id("err"),
        };
        if let Ok(body) = serde_json::to_vec(&error) {
            let _ = write_private(&self.health_path(), &body);
        }
    }

    pub fn clear_error(&self, sent: &Option<HookError>) {
        let Ok(_lock) = self.lock(LOCK_WAIT) else {
            return;
        };
        let stored: Option<HookError> = fs::read(self.health_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        if stored == *sent && sent.is_some() {
            let _ = fs::remove_file(self.health_path());
        }
    }

    /// Best effort after a delivered capture: oldest first, at most 20 events, at most 2 s,
    /// skipping the event just sent. Never an error. The lock covers the read and the final
    /// write only; an event queued meanwhile stays, and one delivered twice is dropped by the
    /// server (same event id). Chat captures wait for a caller that confirmed chat support.
    pub fn replay(&self, config: &Config, agent: &str, skip_event_id: &str, chat_ok: bool) {
        if !self.path.exists() {
            return;
        }
        let batch: Vec<Pending> = {
            let Ok(_lock) = self.lock(Duration::ZERO) else {
                return;
            };
            let Some(queue) = self.read() else { return };
            queue
                .events
                .into_iter()
                .filter(|pending| pending.event_id != skip_event_id)
                .filter(|pending| chat_ok || pending.body["context"].get("chat").is_none())
                .take(REPLAY_LIMIT)
                .collect()
        };
        let deadline = Instant::now() + REPLAY_BUDGET;
        let mut done = HashSet::new();
        let mut failed: Option<(String, String)> = None;
        for pending in batch {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let client = Client::identified(config.clone(), left, agent, ClientKind::Hook);
            let (depth, health) = self.health().unwrap_or_default();
            let mut body = pending.body;
            body["outbox_depth"] = serde_json::json!(depth);
            if let Some(error) = &health {
                body["hook_error_class"] = serde_json::json!(error.class);
                body["hook_error_at"] = serde_json::json!(error.at);
            }
            match client.post(OBSERVE, &body) {
                Ok(_) => {
                    self.clear_error(&health);
                    done.insert(pending.event_id);
                }
                Err(error) => {
                    self.failure(&error);
                    match error {
                        Error::Status { status: 401, .. } => break,
                        error if retryable(&error) => {
                            failed = Some((
                                pending.event_id,
                                error.to_string().chars().take(200).collect(),
                            ));
                            break;
                        }
                        _ => {
                            done.insert(pending.event_id);
                        }
                    }
                }
            }
        }
        if done.is_empty() && failed.is_none() {
            return;
        }
        let Ok(_lock) = self.lock(LOCK_WAIT) else {
            return;
        };
        let Some(mut queue) = self.read() else { return };
        queue.events.retain_mut(|pending| {
            if done.contains(&pending.event_id) {
                return false;
            }
            match &failed {
                Some((id, error)) if *id == pending.event_id => {
                    pending.attempts += 1;
                    pending.last_error = Some(error.clone());
                    pending.attempts < MAX_ATTEMPTS
                }
                _ => true,
            }
        });
        let _ = self.write(&queue);
    }
}
