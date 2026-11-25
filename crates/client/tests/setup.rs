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
        let mut cmd = self.command(agent, remove);
        cmd.env("CLAUDE_CONFIG_DIR", self.dir.join(".claude"));
        with_stdin(&mut cmd, SECRET)
    }

    /// Like `setup`, but without `CLAUDE_CONFIG_DIR`: Claude's files are the default
    /// `~/.claude/settings.json` and `~/.claude.json`.
    fn setup_default_claude(&self, remove: bool) -> Output {
        let mut cmd = self.command("claude", remove);
        cmd.env_remove("CLAUDE_CONFIG_DIR");
        with_stdin(&mut cmd, SECRET)
    }

    fn command(&self, agent: &str, remove: bool) -> std::process::Command {
        let mut cmd = command(&self.dir.join(".config"));
        cmd.env("HOME", &self.dir)
            .env("USERPROFILE", &self.dir)
            .env("PATH", &self.path)
            .env("CODEX_HOME", self.dir.join(".codex"))
            .args(["setup", "http://127.0.0.1:9", "--agent", agent]);
        if remove {
            cmd.arg("--remove");
        }
        cmd
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
fn grok_setup_round_trips_config_hooks_and_instructions() {
    let home = Home::new("grok", false);
    let original = concat!(
        "# Keep this comment\n",
        "[permission]\nallow = [\"Read\"]\ndeny = [\"Bash(rm *)\"]\n",
        "[mcp_servers.other]\ncommand = \"user-mcp\"\n",
    );
    let config = home.write(".grok/config.toml", original);
    let hooks = home.write(".grok/hooks/user.json", r#"{"hooks":{"Stop":[]}}"#);
    let instructions = home.write(".grok/AGENTS.md", "Grok user instructions\n");
    home.ok("grok", false);

    let read_config = || fs::read_to_string(&config).unwrap();
    let installed = read_config().parse::<toml_edit::DocumentMut>().unwrap();
    let server = &installed["mcp_servers"]["cairn"];
    assert_eq!(server["command"].as_str(), Some(EXE));
    assert_eq!(server["args"][0].as_str(), Some("mcp"));
    assert_eq!(server["env"]["CAIRN_SETUP_MANAGED"].as_str(), Some("1"));
    assert_eq!(
        server["env"]["XDG_CONFIG_HOME"].as_str(),
        home.dir.join(".config").to_str()
    );
    let allow = installed["permission"]["allow"].as_array().unwrap();
    assert_eq!(
        allow
            .iter()
            .filter_map(toml_edit::Value::as_str)
            .collect::<Vec<_>>(),
        ["Read", "MCPTool(cairn__*)"]
    );
    let installed = home.json(".grok/hooks/cairn.json");
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "Stop",
        "PreCompact",
        "SessionEnd",
    ] {
        assert_eq!(
            command_of(&installed["hooks"][event][0]),
            expected(event, "grok", &home)
        );
    }
    assert!(
        fs::read_to_string(&instructions)
            .unwrap()
            .contains(cairn_client::tools::instructions())
    );
    check_existing_setup(&home, "grok", ".grok");
    assert_eq!(read_config().trim_end(), original.trim_end());
    assert_eq!(
        fs::read_to_string(hooks).unwrap(),
        r#"{"hooks":{"Stop":[]}}"#
    );
    assert_eq!(
        fs::read_to_string(instructions).unwrap(),
        "Grok user instructions\n"
    );
    assert!(home.json(".grok/hooks/cairn.json").get("hooks").is_none());
}

#[test]
fn antigravity_setup_round_trips_config_hooks_and_instructions() {
    let home = Home::new("antigravity", false);
    let mcp = ".gemini/config/mcp_config.json";
    let hooks = ".gemini/config/hooks.json";
    let settings = ".gemini/antigravity-cli/settings.json";
    let user_mcp = json!({"mcpServers": {"other": {"command": "user-mcp", "disabled": true}}});
    let user_hooks = json!({"user-hook": {"Stop": [{"command": "user-hook"}]}});
    let user_settings = json!({"toolPermission": "request-review", "permissions": {
        "allow": ["mcp(other/*)"], "deny": ["mcp(cairn/memory_forget)"]
    }});
    home.write(mcp, &user_mcp.to_string());
    home.write(hooks, &user_hooks.to_string());
    home.write(settings, &user_settings.to_string());
    let rules = home.write(
        ".gemini/config/AGENTS.md",
        "Antigravity user instructions\n",
    );
    home.ok("antigravity", false);

    let server = &home.json(mcp)["mcpServers"]["cairn"];
    assert_eq!(server["command"], EXE);
    assert_eq!(server["args"], json!(["mcp"]));
    assert_eq!(server["env"]["CAIRN_SETUP_MANAGED"], "1");
    assert_eq!(
        server["env"]["XDG_CONFIG_HOME"],
        json!(home.dir.join(".config"))
    );
    assert_eq!(
        home.json(settings)["permissions"],
        json!({
            "allow": ["mcp(other/*)", "mcp(cairn/*)"], "deny": ["mcp(cairn/memory_forget)"]
        })
    );
    let installed = home.json(hooks);
    for event in ["SessionStart", "PreInvocation", "Stop"] {
        assert_eq!(
            installed["cairn"][event][0]["command"],
            expected(event, "antigravity", &home)
        );
    }
    for event in ["PreToolUse", "PostToolUse"] {
        assert_eq!(
            command_of(&installed["cairn"][event][0]),
            expected(event, "antigravity", &home)
        );
    }
    assert_eq!(
        installed["cairn"]["PreToolUse"][0]["matcher"],
        "^run_command$"
    );
    assert!(
        fs::read_to_string(&rules)
            .unwrap()
            .contains(cairn_client::tools::instructions())
    );
    check_existing_setup(&home, "antigravity", ".gemini");
    assert_eq!(home.json(mcp), user_mcp);
    assert_eq!(home.json(hooks), user_hooks);
    assert_eq!(home.json(settings), user_settings);
    assert_eq!(
        fs::read_to_string(rules).unwrap(),
        "Antigravity user instructions\n"
    );
}

#[test]
fn hermes_setup_preserves_existing_settings_and_removes_only_cairn() {
    let home = Home::new("hermes", false);
    let original = "+model: test-model
approvals:
  mode: manual
plugins:
  enabled: [code-review]
  disabled: [scheduler]
mcp_servers:
  other:
    command: other
    trust: untrusted
";
    let config = home.write(".hermes/config.yaml", original);
    let soul = home.write(".hermes/SOUL.md", "User personality\n");
    let other = home.write(
        ".hermes/plugins/code-review/plugin.yaml",
        "name: code-review\n",
    );
    let load = || yaml_serde::from_str::<Value>(&fs::read_to_string(&config).unwrap()).unwrap();
    let initial = load();
    home.ok("hermes", false);
    let mut installed = load();
    assert_eq!(
        installed["mcp_servers"]
            .as_object_mut()
            .unwrap()
            .remove("cairn")
            .unwrap(),
        json!({
            "command": EXE, "args": ["mcp"], "trust": "full",
            "env": {
                "XDG_CONFIG_HOME": home.dir.join(".config"), "CAIRN_CONFIG": home.config(),
                "CAIRN_SETUP_MANAGED": "1", "CAIRN_SETUP_PLUGIN_ENABLED": "1"
            }
        }),
    );
    assert_eq!(
        installed["plugins"]["enabled"],
        json!(["code-review", "cairn-memory"])
    );
    installed["plugins"]["enabled"] = json!(["code-review"]);
    assert_eq!(installed, initial);
    assert!(
        fs::read_to_string(&soul)
            .unwrap()
            .contains("<!-- cairn:begin -->")
    );
    let plugin = home.dir.join(".hermes/plugins/cairn-memory");
    assert!(
        fs::read_to_string(plugin.join("__init__.py"))
            .unwrap()
            .contains(EXE)
    );
    let manifest = fs::read_to_string(plugin.join("plugin.yaml")).unwrap();
    for event in ["pre_llm_call", "post_tool_call", "transform_tool_result"] {
        assert!(manifest.contains(event), "{event}");
        assert!(
            fs::read_to_string(plugin.join("__init__.py"))
                .unwrap()
                .contains(event),
            "{event}"
        );
    }
    assert!(!manifest.contains("pre_tool_call"));
    assert!(!home.snapshot(".hermes").iter().any(|(_, text)| {
        text.windows(SECRET.len())
            .any(|part| part == SECRET.as_bytes())
    }));

    check_existing_setup(&home, "hermes", ".hermes");
    assert_eq!(load(), initial);
    assert_eq!(fs::read_to_string(&soul).unwrap(), "User personality\n");
    assert_eq!(fs::read_to_string(&other).unwrap(), "name: code-review\n");
    assert!(!plugin.join("__init__.py").exists());
    assert!(!plugin.join("plugin.yaml").exists());
}

#[test]
fn hermes_custom_home_preserves_unowned_entries_and_plugin_preferences() {
    let home = Home::new("hermes-custom", false);
    let profile = home.dir.join("custom profile");
    let config = home.write(
        "custom profile/config.yaml",
        "plugins:\n  enabled: [cairn-memory]\n",
    );
    let run = |remove| {
        let mut cmd = home.command("hermes", remove);
        cmd.env("HERMES_HOME", &profile);
        with_stdin(&mut cmd, SECRET)
    };
    for remove in [false, true] {
        let output = run(remove);
        assert!(output.status.success(), "{output:?}");
    }
    assert!(!home.dir.join(".hermes").exists());
    let read = || fs::read_to_string(&config).unwrap();
    let remaining: Value = yaml_serde::from_str(&read()).unwrap();
    assert_eq!(remaining, json!({"plugins": {"enabled": ["cairn-memory"]}}));

    for (input, expected) in [
        (
            "mcp_servers:\n  cairn:\n    command: user-owned\n",
            "unmarked cairn MCP entry",
        ),
        (
            "plugins:\n  disabled: [cairn-memory]\n",
            "disabled in Hermes plugins.disabled",
        ),
    ] {
        fs::write(&config, input).unwrap();
        let before = home.snapshot("custom profile");
        let output = run(false);
        assert!(
            !output.status.success() && String::from_utf8_lossy(&output.stderr).contains(expected),
            "{output:?}"
        );
        assert_eq!(home.snapshot("custom profile"), before);
    }
}

fn check_existing_setup(home: &Home, agent: &str, folder: &str) {
    let command = || {
        let mut cmd = command(&home.dir.join(".config"));
        cmd.env("HOME", &home.dir).env("USERPROFILE", &home.dir);
        cmd
    };
    let before = home.snapshot(folder);
    assert!(
        command()
            .args(["setup", agent])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(home.snapshot(folder), before);
    let doctor = command().arg("doctor").output().unwrap();
    let doctor = String::from_utf8(doctor.stdout).unwrap();
    assert!(
        doctor.contains(&format!("{agent} MCP entry present")),
        "{doctor}"
    );
    assert!(
        doctor.contains(&format!("{agent} hook present")),
        "{doctor}"
    );
    let removed = command()
        .args(["setup", agent, "--remove"])
        .output()
        .unwrap();
    assert!(removed.status.success(), "{removed:?}");
}

#[test]
fn new_hosts_keep_preexisting_grants_and_refuse_unmarked_servers() {
    let grok = Home::new("grok-existing", false);
    let agy = Home::new("antigravity-existing", false);
    let allow = "[permission]\nallow = [\"MCPTool(cairn__*)\"]\n";
    let toml = grok.write(".grok/config.toml", allow);
    let permissions = r#"{"permissions":{"allow":["mcp(cairn/*)"]}}"#;
    let agy_settings = agy.write(".gemini/antigravity-cli/settings.json", permissions);
    for (home, agent) in [(&grok, "grok"), (&agy, "antigravity")] {
        home.ok(agent, false);
        home.ok(agent, true);
    }
    assert_eq!(
        fs::read_to_string(&toml).unwrap().trim_end(),
        allow.trim_end()
    );
    assert_eq!(fs::read_to_string(agy_settings).unwrap(), permissions);

    let other_grok = "[mcp_servers.cairn]\ncommand = \"user-mcp\"\n";
    fs::write(&toml, other_grok).unwrap();
    let other_agy = r#"{"mcpServers":{"cairn":{"command":"user-mcp"}}}"#;
    let mcp = agy.write(".gemini/config/mcp_config.json", other_agy);
    grok.err("grok", "unmarked cairn MCP entry");
    agy.err("antigravity", "unmarked cairn MCP entry");
    assert_eq!(fs::read_to_string(toml).unwrap(), other_grok);
    assert_eq!(fs::read_to_string(mcp).unwrap(), other_agy);
}

#[test]
fn claude_install_remove_keeps_others() {
    let home = Home::new("claude", false);
    let instructions = "User's Claude instructions.\n";
    let global = home.write(".claude/CLAUDE.md", instructions);
    let other = json!({"matcher": "Read", "hooks": [{"type": "command", "command": "my-helper"}]});
    let permissions = json!({
        "allow": ["WebFetch", "mcp__another__*"],
        "deny": ["Read(./.env)"]
    });
    let original =
        json!({"theme": "dark", "hooks": {"PostToolUse": [other]}, "permissions": permissions});
    home.write(".claude/settings.json", &original.to_string());
    home.write(
        ".claude/.claude.json",
        r#"{"mcpServers":{"other":{"command":"other"}}}"#,
    );
    home.ok("claude", false);

    let settings = home.json(".claude/settings.json");
    assert_eq!(settings.as_object().unwrap().len(), 3);
    assert_eq!(
        settings["permissions"],
        json!({"allow": ["WebFetch", "mcp__another__*", "mcp__cairn"], "deny": ["Read(./.env)"]})
    );
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUseFailure",
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
    assert_eq!(
        settings["hooks"]["PreToolUse"][0]["matcher"],
        "Bash|Write|Edit|MultiEdit|NotebookEdit"
    );
    let block = fs::read_to_string(&global).unwrap();
    assert!(block.starts_with(instructions));
    assert!(block.contains(cairn_client::tools::instructions()));
    let post = settings["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(post[0], other);
    assert_eq!(
        command_of(&post[1]),
        expected("PostToolUse", "claude", &home)
    );
    assert_eq!(
        post[1]["matcher"],
        "Bash|bash|Edit|edit|Write|write|MultiEdit|apply_patch|Read|read|mcp__cairn__memory_save|mcp__cairn__memory_search|mcp__cairn__memory_get|mcp__cairn__memory_feedback"
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
    assert_eq!(fs::read_to_string(global).unwrap(), instructions);
    assert_eq!(home.json(".claude/settings.json"), original);
    assert_eq!(
        home.json(".claude/.claude.json"),
        json!({"mcpServers": {"other": {"command": "other"}}})
    );
}

#[test]
fn claude_native_memory_opt_out_removes_only_cairns_guard() {
    let home = Home::new("claude-keep-native", false);
    let custom = home.dir.join("custom-claude");
    let user = json!({"matcher": "Write", "hooks": [{"type": "command", "command": "user-hook"}]});
    home.write(
        "custom-claude/settings.json",
        &json!({"hooks": {"PreToolUse": [user]}}).to_string(),
    );
    let setup = |option: Option<&str>| {
        let mut cmd = home.command("claude", false);
        cmd.env("CLAUDE_CONFIG_DIR", &custom).args(option);
        let output = with_stdin(&mut cmd, SECRET);
        assert!(output.status.success(), "{output:?}");
    };
    setup(None);
    assert_eq!(
        home.json("custom-claude/settings.json")["hooks"]["PreToolUse"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    setup(Some("--keep-native-memory"));
    let settings = home.json("custom-claude/settings.json");
    assert_eq!(settings["hooks"]["PreToolUse"][0], user);
    assert_eq!(settings["hooks"]["PreToolUse"][1]["matcher"], "Bash");
    assert!(command_of(&settings["hooks"]["PreToolUse"][1]).ends_with(" --keep-native-memory"));
    assert!(command_of(&settings["hooks"]["SessionStart"][0]).ends_with(" --keep-native-memory"));
    assert!(custom.join("CLAUDE.md").is_file());
    assert!(!home.dir.join(".claude/CLAUDE.md").exists());
}

#[test]
fn claude_remove_keeps_allow_array_and_wildcard() {
    for (i, (before, installed, removed)) in [
        (json!([]), json!(["mcp__cairn"]), json!([])),
        (json!(["mcp__cairn"]), json!(["mcp__cairn"]), json!([])),
        (
            json!(["mcp__cairn__*"]),
            json!(["mcp__cairn__*"]),
            json!(["mcp__cairn__*"]),
        ),
        (
            json!(["mcp__cairn", "mcp__cairn__*", "mcp__cairn"]),
            json!(["mcp__cairn", "mcp__cairn__*", "mcp__cairn"]),
            json!(["mcp__cairn__*"]),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let home = Home::new(&format!("claude-permissions-{i}"), false);
        let original = json!({"permissions": {"allow": before}});
        home.write(".claude/settings.json", &original.to_string());

        home.ok("claude", false);
        assert_eq!(
            home.json(".claude/settings.json")["permissions"]["allow"],
            installed
        );
        home.ok("claude", true);
        assert_eq!(
            home.json(".claude/settings.json"),
            json!({"permissions": {"allow": removed}})
        );
    }
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
fn setup_removes_the_0_1_5_run_hook_entries_of_every_host() {
    let home = Home::new("legacy-wrapper", false);
    let run_hook = home.dir.join(".config/cairn/hook/run-hook");
    let legacy = |event: &str| {
        json!([{"hooks": [
            {"type": "command", "command": format!("{} {event}", run_hook.display())},
            {"type": "command", "command": "keep-me"},
        ]}])
    };
    home.write(
        ".claude/settings.json",
        &json!({"hooks": {"Stop": legacy("Stop"), "SessionEnd": legacy("SessionEnd")}}).to_string(),
    );
    // Codex is not configured in this run; its stale entries go too.
    let codex = json!({"hooks": {
        "SessionStart": [{"hooks": [{"type": "command", "command": "~/.config/cairn/hook/run-hook"}]}],
        "Stop": legacy("Stop"),
    }});
    home.write(".codex/hooks.json", &codex.to_string());
    home.write(
        ".config/cairn/hook/run-hook",
        "#!/bin/sh\nexec python3 cairn_hook.py\n",
    );
    home.write(".config/cairn/hook/README.txt", "old hook");

    home.ok("claude", false);

    let settings = home.json(".claude/settings.json");
    assert!(!settings.to_string().contains("run-hook"), "{settings}");
    let stop = settings["hooks"]["Stop"].as_array().unwrap();
    assert_eq!(
        stop[0]["hooks"],
        json!([{"type": "command", "command": "keep-me"}])
    );
    assert_eq!(command_of(&stop[1]), expected("Stop", "claude", &home));
    assert_eq!(
        home.json(".codex/hooks.json"),
        json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "keep-me"}]}]}})
    );
    assert!(!run_hook.exists() && !home.dir.join(".config/cairn/hook").exists());
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
    for event in ["PreToolUse", "Stop"] {
        assert_eq!(
            command_of(&hooks["hooks"][event][0]),
            expected(event, "codex", &home)
        );
    }
    assert_eq!(hooks["hooks"]["PreToolUse"][0]["matcher"], "Bash");
    assert!(
        hooks["hooks"].get("PreCompact").is_none(),
        "Codex has no PreCompact"
    );
    let state = fs::read_to_string(home.dir.join(".codex-fixture-state")).unwrap();
    assert!(
        state.contains(EXE) && state.contains("CAIRN_SETUP_MANAGED"),
        "{state}"
    );
    let config = read(&home, ".codex/config.toml");
    assert!(
        config.contains("default_tools_approval_mode = \"approve\""),
        "{config}"
    );
    let before = home.snapshot(".codex");
    let block = read(&home, ".codex/AGENTS.md");
    assert!(block.contains(cairn_client::tools::instructions()));
    home.ok("codex", false);
    assert_eq!(before, home.snapshot(".codex"));
    let user = "User's Codex instructions.\n";
    let global = home.write(".codex/AGENTS.md", &format!("{user}{block}"));

    // Reuse an existing managed server from before approval was configured.
    let other = "approval_policy = \"on-request\"\n\n[mcp_servers.other]\ncommand = \"other\"\ndefault_tools_approval_mode = \"prompt\"\n";
    let legacy = config.replace("default_tools_approval_mode = \"approve\"\n", "");
    let config_path = home.write(".codex/config.toml", &format!("{other}{legacy}"));
    home.ok("codex", false);
    assert_eq!(
        fs::read_to_string(&config_path).unwrap(),
        format!("{other}{config}")
    );

    home.ok("codex", true);
    home.ok("codex", true);
    assert_eq!(fs::read_to_string(global).unwrap(), user);
    assert!(
        !fs::read_to_string(home.dir.join(".codex/hooks.json"))
            .unwrap()
            .contains("--cairn-managed")
    );
    assert!(!home.dir.join(".codex-fixture-state").exists());
    assert_eq!(
        fs::read_to_string(config_path).unwrap().trim_end(),
        other.trim_end()
    );
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
        "execute.before",
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

fn read(home: &Home, relative: &str) -> String {
    fs::read_to_string(home.dir.join(relative)).unwrap()
}

#[test]
fn claude_setup_edits_only_its_own_bytes_in_the_default_files() {
    // Tab indentation, unsorted keys and numbers serde would rewrite (1e3, 0.10): everything
    // outside our entries must come back byte for byte, also after remove.
    let home = Home::new("claude-bytes", false);
    let settings = "{\n\t\"zeta\": 1e3,\n\t\"alpha\": {\"b\": 2, \"a\": [1, 2]},\n\t\"hooks\": {\n\t\t\"PostToolUse\": [\n\t\t\t{\"matcher\": \"Read\", \"hooks\": [{\"type\": \"command\", \"command\": \"my-helper\"}]}\n\t\t]\n\t}\n}\n";
    let state = "{\n  \"numStartups\": 0.10,\n  \"projects\": {\"/z\": {}, \"/a\": {}},\n  \"mcpServers\": {\n    \"other\": {\"command\": \"x\"}\n  }\n}\n";
    home.write(".claude/settings.json", settings);
    home.write(".claude.json", state);
    let output = home.setup_default_claude(false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (new_settings, new_state) = (
        read(&home, ".claude/settings.json"),
        read(&home, ".claude.json"),
    );
    let at = |text: &str, needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} in {text}"))
    };
    assert!(at(&new_settings, "\"zeta\": 1e3") < at(&new_settings, "\"alpha\""));
    assert!(at(&new_settings, "\"alpha\"") < at(&new_settings, "\"hooks\""));
    assert!(new_settings.contains(
        "{\"matcher\": \"Read\", \"hooks\": [{\"type\": \"command\", \"command\": \"my-helper\"}]}"
    ));
    assert!(new_state.contains("\"numStartups\": 0.10,"));
    assert!(at(&new_state, "\"/z\"") < at(&new_state, "\"/a\""));
    assert!(new_state.contains("\"other\": {\"command\": \"x\"}"));
    let parsed: Value = serde_json::from_str(&new_state).unwrap();
    assert_eq!(parsed["mcpServers"]["cairn"]["command"], EXE);
    assert!(home.dir.join(".claude.json").exists());

    // The same setup again changes nothing.
    let before = (new_settings.clone(), new_state.clone());
    assert!(home.setup_default_claude(false).status.success());
    assert_eq!(
        before,
        (
            read(&home, ".claude/settings.json"),
            read(&home, ".claude.json")
        )
    );

    assert!(home.setup_default_claude(true).status.success());
    let mut remaining = home.json(".claude/settings.json");
    assert_eq!(
        remaining.as_object_mut().unwrap().remove("permissions"),
        Some(json!({"allow": []}))
    );
    assert_eq!(remaining, serde_json::from_str::<Value>(settings).unwrap());
    assert!(read(&home, ".claude/settings.json").contains("\"zeta\": 1e3"));
    assert_eq!(read(&home, ".claude.json"), state);
}

#[test]
fn claude_remove_leaves_nothing_behind_and_creates_nothing() {
    // On a home that never had Cairn: no file, no directory, no empty `hooks`.
    let home = Home::new("claude-empty-remove", false);
    assert!(home.setup_default_claude(true).status.success());
    assert!(!home.dir.join(".claude").exists() && !home.dir.join(".claude.json").exists());

    // Remove the installed entries while retaining an empty allow array.
    home.write(".claude/settings.json", "{\"theme\": \"dark\"}\n");
    home.write(".claude.json", "{\"userID\": \"u1\"}\n");
    assert!(home.setup_default_claude(false).status.success());
    assert!(read(&home, ".claude/settings.json").contains("\"hooks\""));
    assert!(home.setup_default_claude(true).status.success());
    assert_eq!(
        home.json(".claude/settings.json"),
        json!({"theme": "dark", "permissions": {"allow": []}})
    );
    assert_eq!(home.json(".claude.json"), json!({"userID": "u1"}));
}

#[test]
fn remove_still_cleans_the_files_after_the_host_cli_is_gone() {
    let home = Home::new("cli-gone", true);
    for agent in ["codex", "opencode"] {
        home.ok(agent, false);
    }
    assert!(home.dir.join(".codex/hooks.json").exists());
    assert!(
        home.dir
            .join(".config/opencode/plugins/cairn/index.js")
            .exists()
    );
    // `codex` and `opencode` are uninstalled.
    fs::remove_dir_all(home.dir.join("bin")).unwrap();
    fs::create_dir(home.dir.join("bin")).unwrap();
    for agent in ["codex", "opencode"] {
        home.ok(agent, true);
    }
    let hooks = read(&home, ".codex/hooks.json");
    assert!(!hooks.contains("--cairn-managed"), "{hooks}");
    assert!(
        !home
            .dir
            .join(".config/opencode/plugins/cairn/index.js")
            .exists()
    );
    let agents =
        fs::read_to_string(home.dir.join(".config/opencode/AGENTS.md")).unwrap_or_default();
    assert!(!agents.contains("cairn:begin"));
}

#[test]
fn setup_keeps_a_run_hook_that_is_not_the_python_wrapper() {
    let home = Home::new("foreign-wrapper", false);
    home.write(".config/cairn/hook/run-hook", "#!/bin/sh\necho mine\n");
    home.write(".config/cairn/hook/README.txt", "mine too");
    home.ok("claude", false);
    assert!(home.dir.join(".config/cairn/hook/run-hook").exists());
    assert!(home.dir.join(".config/cairn/hook/README.txt").exists());
}

#[test]
fn refresh_adds_command_hooks_to_every_existing_install() {
    // Refresh reads the process environment. Re-enter this test with an isolated home.
    if std::env::var_os("CAIRN_TEST_SETUP_REFRESH").is_some() {
        let lines = cairn_client::setup::refresh(std::path::Path::new(EXE));
        assert_eq!(lines.len(), 6, "{lines:?}");
        assert!(
            lines
                .iter()
                .all(|line| line.ends_with("integration installed")),
            "{lines:?}"
        );
        return;
    }
    let home = Home::new("command-refresh", true);
    let hooks = [
        ("claude", ".claude/settings.json", "hooks"),
        ("codex", ".codex/hooks.json", "hooks"),
        ("grok", ".grok/hooks/cairn.json", "hooks"),
        ("antigravity", ".gemini/config/hooks.json", "cairn"),
    ];
    for (agent, path, group) in hooks {
        home.ok(agent, false);
        let mut legacy = home.json(path);
        let events = legacy[group].as_object_mut().unwrap();
        if agent == "claude" {
            events.get_mut("PreToolUse").unwrap()[0]["matcher"] =
                json!("Write|Edit|MultiEdit|NotebookEdit");
            events.remove("PostToolUseFailure");
        } else {
            events.remove("PreToolUse");
        }
        home.write(path, &legacy.to_string());
    }
    let plugins = [
        (
            "opencode",
            ".config/opencode/plugins/cairn/index.js",
            "execute.before",
        ),
        (
            "hermes",
            ".hermes/plugins/cairn-memory/__init__.py",
            "transform_tool_result",
        ),
    ];
    for (agent, path, _) in plugins {
        home.ok(agent, false);
        let marker = if agent == "opencode" {
            "// cairn:continuity"
        } else {
            "# CAIRN_SETUP_MANAGED"
        };
        home.write(path, &format!("{marker}\n--cairn-managed\n"));
    }
    let mut executable = std::process::Command::new(std::env::current_exe().unwrap());
    for (key, value) in home.command("claude", false).get_envs() {
        match value {
            Some(value) => executable.env(key, value),
            None => executable.env_remove(key),
        };
    }
    executable
        .env("CLAUDE_CONFIG_DIR", home.dir.join(".claude"))
        .env("CAIRN_TEST_SETUP_REFRESH", "1");
    let result = executable
        .args([
            "--exact",
            "refresh_adds_command_hooks_to_every_existing_install",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    for (agent, path, group) in hooks {
        let installed = home.json(path);
        assert_eq!(
            command_of(&installed[group]["PreToolUse"][0]),
            expected("PreToolUse", agent, &home)
        );
        if agent == "claude" {
            assert_eq!(
                installed[group]["PreToolUse"][0]["matcher"],
                "Bash|Write|Edit|MultiEdit|NotebookEdit"
            );
            assert_eq!(
                command_of(&installed[group]["PostToolUseFailure"][0]),
                expected("PostToolUseFailure", agent, &home)
            );
        }
    }
    for (_, path, event) in plugins {
        assert!(read(&home, path).contains(event), "{path}");
    }
}
