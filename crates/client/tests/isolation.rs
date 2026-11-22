//! A test must never touch the developer's own Cairn files. This one re-runs itself with the
//! developer's variables pointing at a "poison" directory, drives the real binary through the
//! shared harness, and fails if anything lands outside the temporary directories.

mod common;

use common::{command, temp_dir, with_stdin};
use std::path::{Path, PathBuf};
use std::process::Command;

const PROBE: &str = "CAIRN_ISOLATION_PROBE";

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        if path.is_dir() {
            stack.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
        } else {
            found.push(path);
        }
    }
    found
}

/// What a hook test does: an unreachable server, so the capture is queued in the outbox.
fn probe() {
    let home = temp_dir("isolation-probe");
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let config = home.join("cairn/config.json");
    cairn_client::Config::new(&url, "cdt_isolationtoken123456")
        .unwrap()
        .save(&config)
        .unwrap();
    let mut hook = command(&home);
    hook.args(["hook", "UserPromptSubmit", "--agent", "claude", "--config"])
        .arg(&config)
        .arg("--cairn-managed");
    let payload = format!(
        r#"{{"session_id":"s1","cwd":"{}","prompt":"hello"}}"#,
        home.display()
    );
    let output = with_stdin(&mut hook, &payload);
    assert_eq!(output.status.code(), Some(0));
    assert!(
        home.join("cairn/outbox.json").exists(),
        "the capture was queued under the temporary home: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Every inherited CAIRN_* variable is removed (or set) on the command itself.
    let envs: Vec<_> = hook.get_envs().collect();
    for (name, _) in
        std::env::vars_os().filter(|(name, _)| name.to_string_lossy().starts_with("CAIRN_"))
    {
        let handled = envs.iter().any(|(set, _)| *set == name.as_os_str());
        assert!(handled, "{name:?} is inherited by the spawned binary");
    }
}

#[test]
fn commands_never_touch_the_developers_cairn_files() {
    if std::env::var_os(PROBE).is_some() {
        return probe();
    }
    let poison = temp_dir("isolation-poison");
    let me = std::env::current_exe().unwrap();
    let output = Command::new(me)
        .args([
            "--exact",
            "commands_never_touch_the_developers_cairn_files",
            "--test-threads=1",
        ])
        .env(PROBE, "1")
        .env("HOME", &poison)
        .env("XDG_CONFIG_HOME", &poison)
        .env("USERPROFILE", &poison)
        .env("CLAUDE_CONFIG_DIR", &poison)
        .env("CODEX_HOME", &poison)
        .env("CAIRN_URL", "http://127.0.0.1:9")
        .env("CAIRN_DEVICE_TOKEN", "cdt_the_developers_real_token")
        .env("CAIRN_CONFIG", poison.join("config.json"))
        .env("CAIRN_OUTBOX", poison.join("outbox.json"))
        .env("CAIRN_CAPTURE_ASSISTANT", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        files_under(&poison),
        Vec::<PathBuf>::new(),
        "a test wrote outside its temporary directory"
    );
}

#[test]
fn the_harness_refuses_a_config_home_outside_the_temporary_directory() {
    let outside = std::panic::catch_unwind(|| command(Path::new("/var/empty/cairn-outside")));
    assert!(outside.is_err());
}
