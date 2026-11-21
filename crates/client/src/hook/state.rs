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
const HASHES_MAX: usize = 512;
const STALE: Duration = Duration::from_secs(7 * 86_400);
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
    /// Brief plus topic notes injected into the current context.
    pub injected_bytes: u32,
    pub topics_injected: u8,
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
    lock_path: PathBuf,
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
        let state = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Ok(Self {
            path,
            lock_path,
            _lock: lock,
            state,
        })
    }

    pub fn save(&self) -> Result<(), String> {
        let body = serde_json::to_vec(&self.state).map_err(|error| error.to_string())?;
        write_private(&self.path, &body)
            .map_err(|error| format!("cannot write {}: {error}", self.path.display()))
    }

    /// SessionEnd: the files go; the lock is released when `self` drops.
    pub fn delete(self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(&self.lock_path);
    }
}

/// Removes state files untouched for seven days (sessions that never ended).
pub fn sweep(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age > STALE);
        if old {
            let _ = fs::remove_file(entry.path());
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
        assert!(!dir.join("k.json").exists() && !dir.join("k.lock").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
