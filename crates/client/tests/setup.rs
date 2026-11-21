//! `cairn setup --agent`: the real binary against a temporary home, with shell stand-ins for the
//! native `codex` and `opencode` CLIs. No test needs Python or a server.

mod common;

use common::{command, temp_dir, with_stdin};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::process::Output;

const EXE: &str = env!("CARGO_BIN_EXE_cairn");
const SECRET: &str = "device-secret-value";

struct Home {
    dir: PathBuf,
    path: std::ffi::OsString,
}

impl Home {
    /// `with_cli`: `codex` and `opencode` stand-ins come first on `PATH`; otherwise `PATH` holds
    /// an empty folder only.
    fn new(name: &str, with_cli: bool) -> Self {
        let dir = temp_dir(&format!("setup-{name}"));
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        if with_cli {
            for cli in ["codex", "opencode"] {
                let file = bin.join(cli);
                fs::write(&file, include_bytes!("fixtures/native_mcp.sh")).unwrap();
                #[cfg(unix)]
                fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                    .unwrap();
            }
        }
        Self {
            path: bin.into_os_string(),
            dir,
        }
    }

    fn config(&self) -> PathBuf {
        self.dir.join(".config/cairn/config.json")
    }

    fn setup(&self, agent: &str, remove: bool) -> Output {
        let mut cmd = command(&self.dir.join(".config"));
        cmd.env("HOME", &self.dir)
            .env("USERPROFILE", &self.dir)
            .env("PATH", &self.path)
            .env("CODEX_HOME", self.dir.join(".codex"))
            .env("CLAUDE_CONFIG_DIR", self.dir.join(".claude"))
            .args(["setup", "http://127.0.0.1:9", "--agent", agent]);
        if remove {
            cmd.arg("--remove");
        }
        with_stdin(&mut cmd, SECRET)
    }

    fn ok(&self, agent: &str, remove: bool) {
        let output = self.setup(agent, remove);
        assert!(
            output.status.success(),
            "{agent}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn err(&self, agent: &str, needle: &str) {
        let output = self.setup(agent, false);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success() && stderr.contains(needle),
            "{stderr}"
        );
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.dir.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    fn json(&self, relative: &str) -> Value {
        serde_json::from_str(&fs::read_to_string(self.dir.join(relative)).unwrap()).unwrap()
    }

    /// Every file under `relative`, by path, for before/after comparison.
    fn snapshot(&self, relative: &str) -> Vec<(PathBuf, Vec<u8>)> {
        let mut found = Vec::new();
        let mut stack = vec![self.dir.join(relative)];
        while let Some(path) = stack.pop() {
            if path.is_dir() {
                stack.extend(fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
            } else {
                found.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
        found.sort();
        found
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn command_of(entry: &Value) -> &str {
    entry["hooks"][0]["command"].as_str().unwrap()
}

fn expected(event: &str, agent: &str, home: &Home) -> String {
    format!(
        "{EXE} hook {event} --agent {agent} --config {} --cairn-managed",
        home.config().display()
    )
}

#[test]
fn claude_install_remove_keeps_others() {
    let home = Home::new("claude", false);
    let other = json!({"matcher": "Read", "hooks": [{"type": "command", "command": "my-helper"}]});
    home.write(
        ".claude/settings.json",
        &json!({"theme": "dark", "hooks": {"PostToolUse": [other]}}).to_string(),
    );
    home.write(
        ".claude/.claude.json",
        r#"{"mcpServers":{"other":{"command":"other"}}}"#,
    );
    home.ok("claude", false);

    let settings = home.json(".claude/settings.json");
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "Stop",
        "PreCompact",
        "SessionEnd",
    ] {
        assert_eq!(
            command_of(&settings["hooks"][event][0]),
            expected(event, "claude", &home),
            "{event}"
        );
    }
    let post = settings["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(post[0], other);
    assert_eq!(
        command_of(&post[1]),
        expected("PostToolUse", "claude", &home)
    );
    assert_eq!(
        post[1]["matcher"],
        "Bash|bash|Edit|edit|Write|write|MultiEdit|apply_patch"
    );
    let mcp = &home.json(".claude/.claude.json")["mcpServers"];
    assert_eq!(mcp["cairn"]["command"], EXE);
    assert_eq!(mcp["cairn"]["args"], json!(["mcp"]));
    assert_eq!(mcp["cairn"]["env"]["CAIRN_SETUP_MANAGED"], "1");
    assert_eq!(mcp["other"], json!({"command": "other"}));
    let text = fs::read_to_string(home.dir.join(".claude/settings.json")).unwrap();
    assert!(!text.contains(SECRET) && !text.contains("python"), "{text}");

    let before = home.snapshot(".claude");
    home.ok("claude", false);
    assert_eq!(before, home.snapshot(".claude"));

    home.ok("claude", true);
    assert_eq!(
        home.json(".claude/settings.json"),
        json!({"theme": "dark", "hooks": {"PostToolUse": [other]}})
    );
    assert_eq!(
        home.json(".claude/.claude.json"),
        json!({"mcpServers": {"other": {"command": "other"}}})
    );
}

#[test]
fn setup_replaces_the_old_python_entries_and_retires_their_files() {
    let home = Home::new("python", false);
    let old = |event: &str| {
        json!([{"hooks": [
            {"type": "command", "command": format!("python3 /x/hook/cairn_hook.py --agent claude --config /y --cairn-managed # {event}")},
            {"type": "command", "command": "keep-me"},
        ]}])
    };
    home.write(
        ".claude/settings.json",
        &json!({"hooks": {"SessionStart": old("s"), "Stop": old("t")}}).to_string(),
    );
    let hook_dir = home.dir.join(".config/cairn/hook");
    let record = |name: &str, body: &str| {
        use sha2::{Digest, Sha256};
        (
            name.to_owned(),
            format!("{:x}", Sha256::digest(body.as_bytes())),
        )
    };
    home.write(".config/cairn/hook/cairn_hook.py", "installed");
    home.write(".config/cairn/hook/cairn_outbox.py", "edited later");
    home.write(
        ".config/cairn/hook/installed.json",
        &json!({record("cairn_hook.py", "installed").0: record("cairn_hook.py", "installed").1,
                "cairn_outbox.py": record("x", "original").1})
        .to_string(),
    );
    home.write(".config/cairn/hook-state.sqlite3", "db");
    home.write(".config/cairn/outbox.json", r#"{"version":2,"events":[]}"#);

    let output = home.setup("claude", false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cairn_outbox.py") && stderr.contains("old Python outbox"),
        "{stderr}"
    );

    let settings = home.json(".claude/settings.json");
    let start = settings["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(start.len(), 2, "{start:?}");
    assert_eq!(
        start[0]["hooks"],
        json!([{"type": "command", "command": "keep-me"}])
    );
    assert_eq!(
        command_of(&start[1]),
        expected("SessionStart", "claude", &home)
    );
    assert!(
        !fs::read_to_string(home.dir.join(".claude/settings.json"))
            .unwrap()
            .contains("cairn_hook.py")
    );
    assert!(
        !hook_dir.join("cairn_hook.py").exists(),
        "the recorded copy is deleted"
    );
    assert!(
        hook_dir.join("cairn_outbox.py").exists(),
        "an edited copy stays"
    );
    assert!(!home.dir.join(".config/cairn/hook-state.sqlite3").exists());
    assert!(
        home.dir.join(".config/cairn/outbox.json").exists(),
        "the v2 queue is untouched"
    );

    // doctor flags a host config that still runs the retired script.
    home.write(
        ".claude/settings.json",
        &json!({"hooks": {"Stop": old("t")}}).to_string(),
    );
    let mut doctor = command(&home.dir.join(".config"));
    doctor
        .env("HOME", &home.dir)
        .env("CLAUDE_CONFIG_DIR", home.dir.join(".claude"))
        .arg("doctor");
    let text = String::from_utf8_lossy(&doctor.output().unwrap().stdout).into_owned();
    assert!(
        text.contains("warn  hooks  claude hook still runs the retired Python script"),
        "{text}"
    );
    home.ok("claude", false);
    let mut doctor = command(&home.dir.join(".config"));
    doctor
        .env("HOME", &home.dir)
        .env("CLAUDE_CONFIG_DIR", home.dir.join(".claude"))
        .arg("doctor");
    let text = String::from_utf8_lossy(&doctor.output().unwrap().stdout).into_owned();
    assert!(text.contains("ok    hooks  claude hook present"), "{text}");
}

#[test]
fn conflict_changes_no_file() {
    let home = Home::new("conflict", true);
    home.write(".claude/settings.json", r#"{"theme":"dark"}"#);
    home.write(
        ".claude/.claude.json",
        r#"{"mcpServers":{"cairn":{"command":"other"}}}"#,
    );
    let before = home.snapshot(".claude");
    home.err("claude", "unmarked");
    assert_eq!(before, home.snapshot(".claude"));

    // No native CLI: nothing is written for codex or opencode.
    let bare = Home::new("bare", false);
    for agent in ["codex", "opencode"] {
        bare.err(agent, "installed");
    }
    assert!(!bare.dir.join(".codex/hooks.json").exists());
    assert!(!bare.dir.join(".config/opencode/AGENTS.md").exists());
}

#[test]
fn unmarked_entries_refused() {
    let home = Home::new("unmarked", true);
    let hooks = home.write(".codex/hooks.json", r#"{"description":"user hooks"}"#);
    home.write(
        ".codex-fixture-state",
        r#"{"command":"user-server","env":{}}"#,
    );
    home.err("codex", "unmarked");
    assert_eq!(
        fs::read_to_string(hooks).unwrap(),
        r#"{"description":"user hooks"}"#
    );

    let raw = r#"{ /* user */ "mcp":{"servers":{"cairn":{"command":["user-server"]},}},}"#;
    let config = home.write(".config/opencode/opencode.jsonc", raw);
    let rules = home.write(".config/opencode/AGENTS.md", "User instructions\n");
    home.err("opencode", "unmarked");
    assert_eq!(fs::read_to_string(config).unwrap(), raw);
    assert_eq!(fs::read_to_string(&rules).unwrap(), "User instructions\n");
    let output = home.setup("opencode", true);
    assert!(
        !output.status.success(),
        "an unmarked entry is refused on remove too"
    );
}

#[test]
fn codex_native_missing_dir() {
    let home = Home::new("codex-dir", true);
    assert!(!home.dir.join(".codex").exists());
    home.ok("codex", false);
    let hooks = home.json(".codex/hooks.json");
    assert_eq!(
        command_of(&hooks["hooks"]["Stop"][0]),
        expected("Stop", "codex", &home)
    );
    assert!(
        hooks["hooks"].get("PreCompact").is_none(),
        "Codex has no PreCompact"
    );
    let state = fs::read_to_string(home.dir.join(".codex-fixture-state")).unwrap();
    assert!(
        state.contains(EXE) && state.contains("CAIRN_SETUP_MANAGED"),
        "{state}"
    );
    let before = home.snapshot(".codex");
    home.ok("codex", false);
    assert_eq!(before, home.snapshot(".codex"));
    home.ok("codex", true);
    home.ok("codex", true);
    assert!(
        !fs::read_to_string(home.dir.join(".codex/hooks.json"))
            .unwrap()
            .contains("--cairn-managed")
    );
    assert!(!home.dir.join(".codex-fixture-state").exists());
}

#[test]
fn opencode_plugin_install_and_remove() {
    let home = Home::new("opencode", true);
    let folder = ".config/opencode";
    home.write(&format!("{folder}/plugin/other.js"), "keep-plugin\n");
    home.write(
        &format!("{folder}/plugin/cairn.js"),
        "// cairn:continuity\nlegacy\n",
    );
    home.write(&format!("{folder}/plugin/stranger.js"), "not cairn\n");
    let raw = r#"{
 // keep
 "plugins": ["other-plugin", {"package":"kept","options":{"cairn":"continuity"}}, {"package":"/tmp/opencode/plugin/cairn.js","options":{"cairn":"continuity"}}],
 "mcp": {"servers": {"other": {"command":["keep"]}}}
}
"#;
    let config = home.write(&format!("{folder}/opencode.jsonc"), raw);
    let rules = home.write(&format!("{folder}/AGENTS.md"), "User instructions\n");
    let index = home.dir.join(folder).join("plugins/cairn/index.js");

    home.ok("opencode", false);
    let text = fs::read_to_string(&config).unwrap();
    let plugin = fs::read_to_string(&index).unwrap();
    for kept in ["// keep", "other-plugin", "kept"] {
        assert!(text.contains(kept), "{kept}: {text}");
    }
    assert!(!text.contains("plugin/cairn.js") && !text.contains(SECRET));
    assert!(plugin.starts_with("// cairn:continuity\n"));
    assert!(
        plugin.contains(&serde_json::to_string(EXE).unwrap()),
        "the plugin names the cairn exe"
    );
    assert!(plugin.contains(&serde_json::to_string(&home.config().to_string_lossy()).unwrap()));
    assert!(
        plugin.contains("\"hook\", payload.hook_event_name") && plugin.contains("--cairn-managed")
    );
    for needle in [
        "CAIRN_CONFIG",
        "session.hook",
        "compaction",
        "execute.after",
    ] {
        assert!(plugin.contains(needle), "{needle}");
    }
    assert!(!plugin.contains("python") && !plugin.contains(SECRET));
    assert!(!home.dir.join(folder).join("plugin/cairn.js").exists());
    assert_eq!(
        fs::read_to_string(home.dir.join(folder).join("plugin/stranger.js")).unwrap(),
        "not cairn\n"
    );
    let agents = fs::read_to_string(&rules).unwrap();
    assert!(agents.contains("User instructions") && agents.contains("cairn:begin"));

    home.ok("opencode", false);
    assert_eq!(text, fs::read_to_string(&config).unwrap());
    assert_eq!(plugin, fs::read_to_string(&index).unwrap());
    fs::write(&index, "user plugin\n").unwrap();
    home.ok("opencode", false);
    assert_eq!(fs::read_to_string(&index).unwrap(), "user plugin\n");
    fs::write(&index, &plugin).unwrap();

    home.ok("opencode", true);
    let removed = fs::read_to_string(&config).unwrap();
    for kept in ["// keep", "other-plugin", "kept", "\"other\""] {
        assert!(removed.contains(kept), "{kept}: {removed}");
    }
    assert!(!removed.contains("plugin/cairn.js") && !removed.contains(SECRET));
    assert!(!index.exists());
    assert_eq!(
        fs::read_to_string(home.dir.join(folder).join("plugin/other.js")).unwrap(),
        "keep-plugin\n"
    );
    assert_eq!(fs::read_to_string(&rules).unwrap(), "User instructions\n");
}

#[test]
fn opencode_setup_writes_only_marked_entries_idempotently() {
    let home = Home::new("opencode-fresh", true);
    home.ok("opencode", false);
    let before = home.snapshot(".config/opencode");
    home.ok("opencode", false);
    assert_eq!(before, home.snapshot(".config/opencode"));
    home.ok("opencode", true);
    home.ok("opencode", true);
    let left: Vec<_> = home.snapshot(".config/opencode");
    for (path, body) in left {
        let text = String::from_utf8_lossy(&body);
        assert!(
            !text.contains("CAIRN_SETUP_MANAGED") && !text.contains("cairn:begin"),
            "{}",
            path.display()
        );
    }
}
