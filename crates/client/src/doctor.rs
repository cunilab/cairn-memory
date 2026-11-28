//! `cairn doctor`: read-only health checks, one `ok|warn|fail  <check>  <detail>` line each.

use crate::{Client, Config, Error, claude_memory, project, selected_config_path, setup};
use cairn_core::capture::redact_sensitive_content as redact;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct Check {
    pub level: &'static str,
    pub name: &'static str,
    pub detail: String,
}

fn check(level: &'static str, name: &'static str, detail: impl Into<String>) -> Check {
    Check {
        level,
        name,
        detail: detail.into(),
    }
}

/// Runs every check; `home` is where the agents keep their config directories.
pub fn run(home: &Path) -> Vec<Check> {
    let mut checks = Vec::new();
    let path = match selected_config_path() {
        Ok(path) => path,
        Err(error) => return vec![check("fail", "config", error.to_string())],
    };
    checks.push(config_check(&path));
    match Config::load() {
        Ok(config) => checks.extend(server_checks(config)),
        Err(error) => checks.push(check("fail", "server", error.to_string())),
    }
    checks.push(project_check());
    checks.extend(agent_checks(home));
    checks
}

fn config_check(path: &Path) -> Check {
    let Ok(meta) = fs::metadata(path) else {
        return check("fail", "config", format!("missing {}", path.display()));
    };
    #[cfg(unix)]
    if std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o077 != 0 {
        return check(
            "warn",
            "config",
            format!("{} is not mode 0600", path.display()),
        );
    }
    let _ = meta;
    check("ok", "config", path.display().to_string())
}

fn server_checks(config: Config) -> Vec<Check> {
    let path = config.path().map(|path| path.display().to_string());
    let client = Client::with_timeout(config, Duration::from_secs(3));
    let health = match client.get("/cairn-memory/health") {
        Ok(health) => health,
        Err(error) => return vec![check("fail", "server", error.to_string())],
    };
    let mut checks = vec![check("ok", "server", "/health 200")];
    checks.push(if health["semantic_search"] == "enabled" {
        check("ok", "semantic", "enabled")
    } else {
        check("warn", "semantic", "disabled; search is keyword only")
    });
    checks.push(match client.get("/cairn-memory/auth/me") {
        Ok(_) => check("ok", "device", "token accepted"),
        Err(Error::Status { status: 401, .. }) => check(
            "fail",
            "device",
            format!(
                "device token rejected (HTTP 401); check {}, run cairn setup",
                path.as_deref().unwrap_or("the selected config")
            ),
        ),
        Err(error) => check("fail", "device", error.to_string()),
    });
    checks
}

fn project_check() -> Check {
    let Ok(cwd) = env::current_dir() else {
        return check("warn", "project", "no current directory");
    };
    if project::toplevel(&cwd).is_none() {
        return check("ok", "project", "not in a git repository");
    }
    match project::read(&cwd) {
        Ok(Some(file)) => {
            // A committed id the server lacks (a wiped or new server) breaks every capture of this
            // checkout, though the file is fine: ask the server. An unreachable one is reported by
            // the server check, so it is not a second failure here.
            let listed = Config::load().ok().and_then(|config| {
                Client::with_timeout(config, Duration::from_secs(3))
                    .get("/cairn-memory/projects")
                    .ok()
            });
            match listed {
                Some(list)
                    if !list.as_array().is_some_and(|projects| {
                        projects.iter().any(|p| p["project_id"] == file.project_id)
                    }) =>
                {
                    check(
                        "fail",
                        "project",
                        format!(
                            "{} is not on the server (wiped or new?) or you are not a member; run cairn init to register or join it",
                            file.project_id
                        ),
                    )
                }
                _ => check("ok", "project", file.project_id),
            }
        }
        Ok(None) => check(
            "warn",
            "project",
            "no .cairn/project.json: project memories are hidden and saves are personal; run cairn init",
        ),
        Err(error) => check("fail", "project", error.to_string()),
    }
}

/// Check MCP and hooks for detected agents, and instruction drift for Cairn-managed installs.
fn agent_checks(home: &Path) -> Vec<Check> {
    let dir = |var: &str, default: PathBuf| {
        env::var_os(var)
            .filter(|value| !value.is_empty())
            .map_or(default, PathBuf::from)
    };
    let claude = claude_memory::config_dir(home);
    let claude_json = if env::var_os("CLAUDE_CONFIG_DIR").is_some_and(|dir| !dir.is_empty()) {
        claude.join(".claude.json")
    } else {
        home.join(".claude.json")
    };
    let codex = dir("CODEX_HOME", home.join(".codex"));
    let opencode = dir("XDG_CONFIG_HOME", home.join(".config")).join("opencode");
    let grok = setup::grok::config_dir(home);
    let antigravity = setup::antigravity::config_dir(home);
    let hermes = setup::hermes::config_dir(home);
    // (agent, config dir, MCP files, hook file, instruction filename)
    let agents = [
        (
            "claude",
            claude.clone(),
            vec![claude_json],
            claude.join("settings.json"),
            "CLAUDE.md",
        ),
        (
            "codex",
            codex.clone(),
            vec![codex.join("config.toml")],
            codex.join("hooks.json"),
            "AGENTS.md",
        ),
        (
            "opencode",
            opencode.clone(),
            vec![
                opencode.join("opencode.json"),
                opencode.join("opencode.jsonc"),
            ],
            opencode.join("plugins/cairn/index.js"),
            "AGENTS.md",
        ),
        (
            "grok",
            grok.clone(),
            vec![grok.join("config.toml")],
            setup::grok::hook_file(home),
            "AGENTS.md",
        ),
        (
            "antigravity",
            antigravity.clone(),
            vec![antigravity.join("mcp_config.json")],
            setup::antigravity::hook_file(home),
            "AGENTS.md",
        ),
        (
            "hermes",
            hermes.clone(),
            vec![hermes.join("config.yaml")],
            setup::hermes::hook_file(home),
            "SOUL.md",
        ),
    ];
    let mut checks = Vec::new();
    for (agent, folder, mcp, hook, instruction_name) in &agents {
        if !folder.is_dir() {
            continue;
        }
        // Hermes uses Cairn's memory provider tools instead of an MCP entry.
        let managed_mcp = if *agent == "hermes" {
            setup::hermes::provider_set(&mcp[0])
        } else {
            mcp.iter()
                .any(|file| fs::read_to_string(file).is_ok_and(|text| text.contains(setup::MARKER)))
        };
        let tools = if *agent == "hermes" {
            "memory provider"
        } else {
            "MCP entry"
        };
        let hook_text = fs::read_to_string(hook);
        let managed_hook = hook_text
            .as_ref()
            .is_ok_and(|text| text.contains(setup::MANAGED_FLAG));
        checks.push(if managed_mcp && *agent == "hermes" && setup::hermes::gateway_profile(&mcp[0]) {
            // Doctor reads files: a gateway process that was running during setup still lacks the chat hook.
            check(
                "ok",
                "agents",
                "hermes gateway files present; a gateway process loads them only when it starts",
            )
        } else if managed_mcp && *agent == "hermes" {
            check(
                "ok",
                "agents",
                "hermes memory provider present; a running Hermes or Hermes gateway process loads it only when it starts",
            )
        } else if managed_mcp {
            check("ok", "agents", format!("{agent} {tools} present"))
        } else {
            check(
                "warn",
                "agents",
                format!("{agent} has no Cairn {tools}; run cairn setup"),
            )
        });
        checks.push(
            if hook_text
                .as_ref()
                .is_ok_and(|text| text.contains("cairn_hook.py") || text.contains("hook/run-hook"))
            {
                check(
                    "warn",
                    "hooks",
                    format!("{agent} hook still runs the retired Python script; run cairn setup"),
                )
            } else if managed_hook {
                check("ok", "hooks", format!("{agent} hook present"))
            } else {
                check(
                    "warn",
                    "hooks",
                    format!("{agent} has no Cairn hook; run cairn setup"),
                )
            },
        );
        if !managed_mcp && !managed_hook {
            continue;
        }
        if *agent == "hermes"
            && let Some(warning) = setup::hermes::version_warning(home)
        {
            checks.push(check("warn", "hermes", warning));
        }
        let instruction = folder.join(instruction_name);
        if !(*agent == "hermes" && setup::hermes::gateway_profile(&folder.join("config.yaml")))
            && let Some(issue) = setup::instruction_issue(&instruction)
        {
            checks.push(check(
                "warn",
                "instructions",
                format!(
                    "{agent} {}: {issue}; run cairn setup {agent}",
                    redact(&instruction.display().to_string())
                ),
            ));
        }
        if *agent == "claude"
            && !setup::claude::keeps_native_memory(hook)
            && let Ok(cwd) = env::current_dir()
            && let Some(index) = claude_memory::index_without_pointer(&cwd, home)
        {
            checks.push(check(
                "warn",
                "claude-memory",
                format!(
                    "{} is missing the Cairn pointer; run cairn import claude-memory",
                    redact(&index.display().to_string())
                ),
            ));
        }
    }
    checks
}

pub fn print(checks: &[Check]) -> bool {
    for check in checks {
        println!("{:<4}  {}  {}", check.level, check.name, check.detail);
    }
    checks.iter().any(|check| check.level == "fail")
}
