//! `cairn doctor`: read-only health checks, one `ok|warn|fail  <check>  <detail>` line each.

use crate::{Client, Config, Error, project, selected_config_path};
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
        Ok(Some(file)) => check("ok", "project", file.project_id),
        Ok(None) => check(
            "warn",
            "project",
            "no .cairn/project.json: project memories are hidden and saves are personal; run cairn init",
        ),
        Err(error) => check("fail", "project", error.to_string()),
    }
}

/// Agents whose config directory exists; each must carry a Cairn-managed MCP entry and hook.
/// Paths and markers mirror `scripts/cairn_setup.py`.
fn agent_checks(home: &Path) -> Vec<Check> {
    let dir = |var: &str, default: PathBuf| env::var_os(var).map_or(default, PathBuf::from);
    let claude = dir("CLAUDE_CONFIG_DIR", home.join(".claude"));
    let claude_json = if env::var_os("CLAUDE_CONFIG_DIR").is_some() {
        claude.join(".claude.json")
    } else {
        home.join(".claude.json")
    };
    let codex = dir("CODEX_HOME", home.join(".codex"));
    let opencode = dir("XDG_CONFIG_HOME", home.join(".config")).join("opencode");
    // (agent, config dir, file holding the MCP entry, files holding the hook command)
    let agents = [
        (
            "claude",
            claude.clone(),
            vec![claude_json],
            vec![claude.join("settings.json")],
        ),
        (
            "codex",
            codex.clone(),
            vec![codex.join("config.toml")],
            vec![codex.join("hooks.json")],
        ),
        (
            "opencode",
            opencode.clone(),
            vec![
                opencode.join("opencode.json"),
                opencode.join("opencode.jsonc"),
            ],
            vec![opencode.join("plugins/cairn/index.js")],
        ),
    ];
    let has = |files: &[PathBuf], marker: &str| {
        files
            .iter()
            .any(|file| fs::read_to_string(file).is_ok_and(|text| text.contains(marker)))
    };
    let mut checks = Vec::new();
    for (agent, folder, mcp, hooks) in &agents {
        if !folder.is_dir() {
            continue;
        }
        checks.push(if has(mcp, "CAIRN_SETUP_MANAGED") {
            check("ok", "agents", format!("{agent} MCP entry present"))
        } else {
            check(
                "warn",
                "agents",
                format!("{agent} has no Cairn MCP entry; run cairn setup"),
            )
        });
        checks.push(if has(hooks, "--cairn-managed") {
            check("ok", "hooks", format!("{agent} hook present"))
        } else {
            check(
                "warn",
                "hooks",
                format!("{agent} has no Cairn hook; run cairn setup"),
            )
        });
    }
    checks
}

pub fn print(checks: &[Check]) -> bool {
    for check in checks {
        println!("{:<4}  {}  {}", check.level, check.name, check.detail);
    }
    checks.iter().any(|check| check.level == "fail")
}
