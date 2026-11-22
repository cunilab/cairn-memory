//! Per-session hook state: delivery flags, repeat hashes and injection counters, never memory
//! text or credentials. One JSON file per session under `<config_dir>/hook-state/`, changed
//! only while holding `<file>.lock` (CM-SPEC-006 §5).

use crate::{create_private_dir, write_private};
use cairn_core::id::sha256_hex;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const TOPICS_MAX: usize = 64;
const EDITED_MAX: usize = 64;
const HASHES_MAX: usize = 512;
const STALE: Duration = Duration::from_secs(7 * 86_400);
const LOCK_STALE: Duration = Duration::from_secs(86_400);
const LOCK_WAIT: Duration = Duration::from_secs(1);

#[derive(Default, Serialize, Deserialize)]
pub struct SessionState {
    /// Meaningful work since the last stored checkpoint.
    pub dirty: bool,
    /// Meaningful work at any point in the session.
    pub worked: bool,
    /// Bumped by every meaningful tool call; a checkpoint clears `dirty` only if unchanged.
    pub generation: u64,
    /// Paths already looked up for topic notes.
    pub topics: Vec<String>,
    pub capture_hashes: Vec<String>,
    /// Topic notes injected so far; the cap is `inject::TOPICS_PER_SESSION`.
    pub topics_injected: u8,
    /// Repo files edited in this session (distinct, at most `EDITED_MAX`), for the Stop reminder.
    #[serde(default)]
    pub edited: Vec<String>,
    /// A recognised test command ran, or a `Decision:`-style line was written.
    #[serde(default)]
    pub worked_signal: bool,
    /// The agent saved to Cairn (the `memory_save` tool or `cairn save`).
    #[serde(default)]
    pub saved: bool,
    /// The Stop reminder text the server gave at session start; the built-in one when none.
    #[serde(default)]
    pub stop_nudge: Option<String>,
    /// The Stop reminder was sent; it is sent once per session.
    #[serde(default)]
    pub nudged: bool,
    /// Topic lookups sent so far; the cap is `inject::TOPIC_LOOKUPS_PER_SESSION`.
    #[serde(default)]
    pub topic_lookups: u8,
}

fn push_capped(list: &mut Vec<String>, value: &str, cap: usize) {
    list.push(value.to_owned());
    if list.len() > cap {
        list.remove(0);
    }
}

impl SessionState {
    pub fn remember_topic(&mut self, path: &str) {
        if !self.topics.iter().any(|seen| seen == path) {
            push_capped(&mut self.topics, path, TOPICS_MAX);
        }
    }

    pub fn remember_edit(&mut self, path: &str) {
        if !self.edited.iter().any(|seen| seen == path) && self.edited.len() < EDITED_MAX {
            self.edited.push(path.to_owned());
        }
    }

    /// `false` when this exact capture was already sent in this session.
    pub fn remember_capture(&mut self, hash: &str) -> bool {
        if self.capture_hashes.iter().any(|seen| seen == hash) {
            return false;
        }
        push_capped(&mut self.capture_hashes, hash, HASHES_MAX);
        true
    }
}

/// sha256(origin, token, agent, session)[..32]: another server, device or agent never shares a file.
pub fn key(origin: &str, token: &str, agent: &str, session_id: &str) -> String {
    sha256_hex(&[origin, token, agent, session_id])[..32].to_owned()
}

/// The state of one session, locked until dropped.
pub struct Locked {
    path: PathBuf,
    /// The state as read, to skip writing a visit that changed nothing.
    original: Vec<u8>,
    _lock: File,
    pub state: SessionState,
}

impl Locked {
    /// Waits up to a second for the lock; a corrupt file starts over empty.
    pub fn open(dir: &Path, key: &str) -> Result<Self, String> {
        create_private_dir(dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        let lock_path = dir.join(format!("{key}.lock"));
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).write(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let lock = options
            .open(&lock_path)
            .map_err(|error| format!("cannot open {}: {error}", lock_path.display()))?;
        let started = std::time::Instant::now();
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) if started.elapsed() < LOCK_WAIT => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return Err("hook state is busy".to_owned()),
            }
        }
        let path = dir.join(format!("{key}.json"));
        let state: SessionState = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let original = serde_json::to_vec(&state).unwrap_or_default();
        Ok(Self {
            path,
            original,
            _lock: lock,
            state,
        })
    }

    /// Writes the state unless it is what was read.
    pub fn save(&self) -> Result<(), String> {
        let body = serde_json::to_vec(&self.state).map_err(|error| error.to_string())?;
        if body == self.original {
            return Ok(());
        }
        write_private(&self.path, &body)
            .map_err(|error| format!("cannot write {}: {error}", self.path.display()))
    }

    /// SessionEnd: the state goes. The lock file stays (another process may be waiting on it);
    /// `sweep` removes it later.
    pub fn delete(self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn age(path: &Path) -> Option<Duration> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    SystemTime::now().duration_since(modified).ok()
}

/// Removes the files of sessions that never ended: a state file untouched for seven days, and
/// with it its lock; a lock whose state is gone only once it is a day old (a session that just
/// started has a lock and no state yet, and an active old session keeps its lock because its
/// state file is fresh).
pub fn sweep(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_lock = path.extension().is_some_and(|ext| ext == "lock");
        let state = path.with_extension("json");
        let remove = if is_lock {
            match age(&state) {
                Some(state_age) => state_age > STALE,
                None => age(&path).is_some_and(|lock_age| lock_age > LOCK_STALE),
            }
        } else {
            age(&path).is_some_and(|state_age| state_age > STALE)
        };
        if remove {
            let _ = fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cairn-hook-state-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn lists_are_fifo_capped_and_repeats_are_refused() {
        let mut state = SessionState::default();
        for index in 0..HASHES_MAX + 3 {
            assert!(state.remember_capture(&index.to_string()));
        }
        assert_eq!(state.capture_hashes.len(), HASHES_MAX);
        assert!(state.remember_capture("0"), "the oldest was evicted");
        assert!(!state.remember_capture("5"));
        for index in 0..TOPICS_MAX + 2 {
            state.remember_topic(&format!("f{index}"));
        }
        state.remember_topic("f10");
        assert_eq!(state.topics.len(), TOPICS_MAX);
        assert_eq!(state.topics[0], "f2");
    }

    #[test]
    fn state_round_trips_privately_and_the_lock_excludes_a_second_holder() {
        let dir = dir("lock");
        let mut first = Locked::open(&dir, "k").unwrap();
        first.state.generation = 7;
        first.save().unwrap();
        let second = File::open(dir.join("k.lock")).unwrap();
        assert!(matches!(second.try_lock(), Err(TryLockError::WouldBlock)));
        drop(second);
        drop(first);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode =
                |name: &str| fs::metadata(dir.join(name)).unwrap().permissions().mode() & 0o777;
            assert_eq!((mode("k.json"), mode("k.lock")), (0o600, 0o600));
        }
        let again = Locked::open(&dir, "k").unwrap();
        assert_eq!(again.state.generation, 7);
        again.delete();
        assert!(!dir.join("k.json").exists());
        assert!(
            dir.join("k.lock").exists(),
            "the lock file stays: another process may wait on it"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unchanged_state_is_not_rewritten() {
        let dir = dir("unchanged");
        Locked::open(&dir, "k").unwrap().save().unwrap();
        assert!(
            !dir.join("k.json").exists(),
            "a read-only visit writes nothing"
        );
        let mut locked = Locked::open(&dir, "k").unwrap();
        locked.state.generation = 1;
        locked.save().unwrap();
        assert!(dir.join("k.json").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweep_keeps_what_an_active_session_still_uses() {
        let dir = dir("sweep");
        fs::create_dir_all(&dir).unwrap();
        let age = |name: &str, days: u64| {
            let file = File::options()
                .write(true)
                .create(true)
                .truncate(false)
                .open(dir.join(name))
                .unwrap();
            file.set_modified(SystemTime::now() - Duration::from_secs(days * 86_400))
                .unwrap();
        };
        // An active session older than seven days is told apart by its fresh state file.
        age("live.json", 0);
        age("live.lock", 30);
        // A finished session leaves an old lock and no state; a forgotten one leaves both old.
        age("done.lock", 2);
        age("lost.json", 30);
        age("lost.lock", 30);
        // A lock whose state is gone but which was just created is a session being started.
        age("new.lock", 0);
        sweep(&dir);
        let left = |name: &str| dir.join(name).exists();
        assert!(left("live.json") && left("live.lock"));
        assert!(!left("done.lock") && !left("lost.json") && !left("lost.lock"));
        assert!(left("new.lock"));
        let _ = fs::remove_dir_all(&dir);
    }
}
