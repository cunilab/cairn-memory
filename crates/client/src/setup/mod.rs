//! `cairn setup --agent`: installs or removes Cairn's MCP entry, permissions, instructions and
//! hooks in supported agents (apps/docs/content/users/agents/index.md#setup). Cairn marks its own
//! MCP registrations and hooks. Setup checks for conflicts before running a native MCP command
//! and applying the file edits. Credentials stay in the Cairn config.

pub(crate) mod antigravity;
pub(crate) mod claude;
mod codex;
pub(crate) mod grok;
pub(crate) mod hermes;
mod jsonc;
mod opencode;

use crate::{Error, create_private_dir, selected_config_path, write_private};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::{env, fs, io};

pub use cairn_core::account::AGENTS;
pub(crate) const MARKER: &str = "CAIRN_SETUP_MANAGED";
// The host can rewrite its config. Keep grant ownership with the marked MCP registration.
const APPROVAL_MARKER: &str = "CAIRN_SETUP_APPROVAL";
pub(crate) const MANAGED_FLAG: &str = "--cairn-managed";
pub(crate) const KEEP_NATIVE_MEMORY: &str = "--keep-native-memory";
pub(crate) const UNMARKED: &str =
    "an unmarked cairn MCP entry exists; rename or remove it explicitly";
const NATIVE_TIMEOUT: Duration = Duration::from_secs(20);

type R<T> = Result<T, String>;

/// Files to write, an optional native MCP command, and a follow-up step that can read its output files.
struct Plan {
    files: Vec<(PathBuf, String)>,
    native: Option<Vec<String>>,
    after: Option<After>,
}

type After = Box<dyn FnOnce(&mut Vec<(PathBuf, String)>) -> R<()>>;

pub(crate) struct Ctx<'a> {
    agent: &'a str,
    remove: bool,
    exe: &'a str,
    config: &'a Path,
    home: PathBuf,
    keep_native_memory: bool,
    gateway: bool,
}

impl Ctx<'_> {
    /// `$VAR` when it is set and not empty.
    fn dir(&self, var: &str) -> Option<PathBuf> {
        env::var_os(var)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }

    /// The MCP server entry env; the marker lets later runs recognise the entry as ours.
    fn mcp_env(&self) -> Map<String, Value> {
        let base = self
            .config
            .parent()
            .and_then(Path::parent)
            .unwrap_or(Path::new("."));
        let mut env = Map::new();
        env.insert("XDG_CONFIG_HOME".into(), json!(base));
        env.insert(MARKER.into(), json!("1"));
        env
    }

    /// `<exe> hook <Event> --agent <a> --config <cfg> --cairn-managed`, shell quoted.
    fn hook_command(&self, event: &str) -> String {
        let config = self.config.to_string_lossy();
        let mut command = [
            self.exe,
            "hook",
            event,
            "--agent",
            self.agent,
            "--config",
            &config,
            MANAGED_FLAG,
        ]
        .map(quote)
        .join(" ");
        // Remember the opt-out on an owned handler so refresh can preserve it.
        if self.keep_native_memory && matches!(event, "SessionStart" | "PreToolUse") {
            command.push(' ');
            command.push_str(KEEP_NATIVE_MEMORY);
        }
        command
    }
}

/// POSIX shell quoting of one word (`shlex.quote`).
fn quote(word: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c);
    if !word.is_empty() && word.chars().all(safe) {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r#"'"'"'"#))
    }
}

/// An entry is ours when its env (`env`, or `environment` for OpenCode) carries the marker.
pub(crate) fn owned(entry: &Value) -> bool {
    let env = entry.get("environment").or_else(|| entry.get("env"));
    env.and_then(|env| env.get(MARKER)) == Some(&json!("1"))
}

pub(crate) fn read(path: &Path) -> R<String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

/// `path` as a host JSON file: edits run on its text by span (`jsonc::put`, `push`, `delete`),
/// so every other byte, key order and number spelling stays. `change` gets the text (`{}` for a
/// missing or empty file) and the parsed root; an edit that changes nothing returns the file as
/// it is, so a no-op never creates it.
pub(crate) fn edit_json(
    path: &Path,
    change: impl FnOnce(&str, &Map<String, Value>) -> R<String>,
) -> R<String> {
    let raw = read(path)?;
    let base = if raw.trim().is_empty() { "{}\n" } else { &raw };
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let root = match serde_json::from_str(base) {
        Ok(Value::Object(map)) => map,
        Ok(_) => return Err(format!("{name} must contain a JSON object")),
        Err(error) => return Err(format!("{name} is not valid JSON: {error}")),
    };
    let edited = change(base, &root)?;
    Ok(if edited == base { raw } else { edited })
}

/// The `mcpServers.cairn` entry of a Claude file: set, or removed. An entry that is not ours is
/// refused, also on removal.
pub(crate) fn mcp_text(path: &Path, entry: &Value, remove: bool) -> R<String> {
    edit_json(path, |text, root| {
        let servers = match root.get("mcpServers") {
            None => None,
            Some(Value::Object(servers)) => Some(servers),
            Some(_) => return Err("mcpServers must be an object".into()),
        };
        if servers
            .and_then(|servers| servers.get("cairn"))
            .is_some_and(|existing| !owned(existing))
        {
            return Err(UNMARKED.into());
        }
        let path = ["mcpServers", "cairn"];
        if remove {
            jsonc::delete(text, &path)
        } else {
            jsonc::put(text, &path, entry)
        }
    })
}

/// The 0.1.5 hook entries ran `<config dir>/hook/run-hook`, a wrapper with no
/// `--cairn-managed` flag that execs the deleted Python script.
fn legacy_run_hook(handler: &Value) -> bool {
    let command = handler
        .get("command")
        .map_or(String::new(), Value::to_string);
    command.contains("cairn/hook/run-hook") || command.contains("cairn\\\\hook\\\\run-hook")
}

/// Replaces the Cairn hook handlers of every event; other handlers and groups stay. The
/// `--cairn-managed` marker also finds the retired Python hook's entries, and the legacy
/// `run-hook` path its 0.1.5 wrapper's, so a re-setup replaces them. Only the touched groups
/// change in the file.
pub(crate) fn hooks_text(path: &Path, ctx: &Ctx, events: &[(&str, u32)]) -> R<String> {
    let ours = |handler: &Value| {
        legacy_run_hook(handler)
            || handler
                .get("command")
                .map_or(String::new(), Value::to_string)
                .contains(MANAGED_FLAG)
    };
    let wanted = |event: &str| match (ctx.remove, events.iter().find(|e| e.0 == event)) {
        (false, Some((_, timeout))) => {
            let handler =
                json!({"type": "command", "command": ctx.hook_command(event), "timeout": timeout});
            let mut group = json!({"hooks": [handler]});
            if event == "PostToolUse" && ctx.agent != "grok" {
                group["matcher"] = json!(
                    "Bash|bash|Edit|edit|Write|write|MultiEdit|apply_patch|Read|read|mcp__cairn__memory_save|mcp__cairn__memory_search|mcp__cairn__memory_get|mcp__cairn__memory_feedback"
                );
            }
            if event == "PreToolUse" {
                group["matcher"] = json!(match ctx.agent {
                    "claude" if !ctx.keep_native_memory =>
                        format!("Bash|{}", crate::claude_memory::WRITE_TOOLS),
                    _ => "Bash".to_owned(),
                });
            }
            if event == "PostToolUseFailure" {
                group["matcher"] = json!("Bash");
            }
            Some(group)
        }
        _ => None,
    };
    edit_hooks(path, &ours, &wanted)
}

/// Drops the handlers `ours` matches from every event and appends the `wanted` group, if any.
fn edit_hooks(
    path: &Path,
    ours: &dyn Fn(&Value) -> bool,
    wanted: &dyn Fn(&str) -> Option<Value>,
) -> R<String> {
    const ALL: [&str; 8] = [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PostToolUseFailure",
        "Stop",
        "PreCompact",
        "SessionEnd",
    ];
    edit_json(path, |text, root| {
        let none = Map::new();
        let hooks = match root.get("hooks") {
            None => &none,
            Some(Value::Object(hooks)) => hooks,
            Some(_) => return Err("hooks must be an object".into()),
        };
        let mut text = text.to_owned();
        for event in ALL {
            let groups = match hooks.get(event) {
                None => &[][..],
                Some(Value::Array(groups)) => groups,
                Some(_) => return Err(format!("{event} must be an array")),
            };
            let (mut rewritten, mut dropped) = (Vec::new(), Vec::new());
            for (index, group) in groups.iter().enumerate() {
                let handlers = group.get("hooks").and_then(Value::as_array);
                let (true, Some(handlers)) = (group.is_object(), handlers) else {
                    return Err(format!("invalid {event} hook group"));
                };
                if handlers.iter().any(|h| !h.is_object()) {
                    return Err(format!("invalid {event} hook handler"));
                }
                let others: Vec<Value> = handlers.iter().filter(|h| !ours(h)).cloned().collect();
                if others.len() == handlers.len() {
                    continue;
                }
                if others.is_empty() {
                    dropped.push(index);
                } else {
                    rewritten.push((index, others));
                }
            }
            let wanted = wanted(event);
            // Already exactly ours: leave the file's bytes alone.
            if rewritten.is_empty()
                && dropped.len() == usize::from(wanted.is_some())
                && wanted.as_ref() == dropped.first().map(|index| &groups[*index])
            {
                continue;
            }
            for (index, others) in rewritten {
                let at = ["hooks", event, &index.to_string(), "hooks"];
                text = jsonc::put(&text, &at, &Value::Array(others))?;
            }
            for index in dropped.iter().rev() {
                text = jsonc::delete(&text, &["hooks", event, &index.to_string()])?;
            }
            if let Some(group) = wanted {
                text = jsonc::push(&text, &["hooks", event], &group)?;
            }
        }
        Ok(text)
    })
}

const INSTRUCTION_BEGIN: &str = "<!-- cairn:begin -->";
const INSTRUCTION_END: &str = "<!-- cairn:end -->";

fn instruction_span(text: &str) -> Result<Option<Range<usize>>, &'static str> {
    let mut begins = text.match_indices(INSTRUCTION_BEGIN);
    let mut ends = text.match_indices(INSTRUCTION_END);
    match (begins.next(), ends.next()) {
        (None, None) => Ok(None),
        (Some((start, _)), Some((end, _)))
            if start < end && begins.next().is_none() && ends.next().is_none() =>
        {
            Ok(Some(start..end + INSTRUCTION_END.len()))
        }
        _ => Err("malformed Cairn markers; repair the marked block first"),
    }
}

fn instruction_block() -> String {
    let content = crate::tools::instructions().trim_end();
    format!("{INSTRUCTION_BEGIN}\n{content}\n{INSTRUCTION_END}")
}

pub(crate) fn instruction_issue(path: &Path) -> Option<&'static str> {
    let Ok(text) = read(path) else {
        return Some("could not read the Cairn instruction file");
    };
    match instruction_span(&text) {
        Ok(Some(span)) => (text[span] != instruction_block())
            .then_some("Cairn instruction block is edited or stale"),
        Ok(None) => Some("Cairn instruction block is missing"),
        Err(problem) => Some(problem),
    }
}

fn instruction_file(path: PathBuf, remove: bool) -> R<(PathBuf, String)> {
    let mut text = read(&path)?;
    if let Some(mut span) = instruction_span(&text)? {
        if text[span.end..].starts_with('\n') {
            span.end += 1;
        }
        text.replace_range(span, "");
    }
    if !remove {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&instruction_block());
        text.push('\n');
    }
    Ok((path, text))
}

/// The executable of a native agent CLI on `PATH`.
fn which(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|file| file.is_file())
}

pub(crate) fn require_cli(agent: &str) -> R<PathBuf> {
    which(agent)
        .ok_or_else(|| format!("the installed {agent} CLI is required for agent setup/removal"))
}

/// Runs `<agent> mcp <args>` for at most 20 s and returns its stdout.
pub(crate) fn native(agent: &str, args: &[String], home: &Path) -> R<String> {
    let exe = require_cli(agent)?;
    if agent == "codex" {
        let folder = env::var_os("CODEX_HOME")
            .filter(|v| !v.is_empty())
            .map_or(home.join(".codex"), PathBuf::from);
        create_private_dir(&folder)
            .map_err(|e| format!("cannot create {}: {e}", folder.display()))?;
    }
    let fail = || format!("{agent} MCP command could not complete");
    let mut child = Command::new(exe)
        .arg("mcp")
        .args(args)
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| fail())?;
    let stdout = child.stdout.take().ok_or_else(fail)?;
    let reader = std::thread::spawn(move || io::read_to_string(stdout));
    let deadline = Instant::now() + NATIVE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(fail());
            }
        }
    };
    let output = reader.join().ok().and_then(Result::ok).unwrap_or_default();
    if status.success() {
        Ok(output)
    } else {
        Err(format!(
            "{agent} MCP command failed; check the native CLI configuration"
        ))
    }
}

/// `--env K=V ... -- <exe> mcp`, the tail of a native `mcp add`.
pub(crate) fn add_args(mut head: Vec<String>, ctx: &Ctx) -> Vec<String> {
    for (key, value) in ctx.mcp_env() {
        head.extend([
            "--env".to_owned(),
            format!("{key}={}", value.as_str().unwrap_or_default()),
        ]);
    }
    head.extend(["--".to_owned(), ctx.exe.to_owned(), "mcp".to_owned()]);
    head
}

fn plan(ctx: &Ctx) -> R<Plan> {
    match ctx.agent {
        "claude" => claude::plan(ctx),
        "codex" => codex::plan(ctx),
        "opencode" => opencode::plan(ctx),
        "grok" => grok::plan(ctx),
        "antigravity" => antigravity::plan(ctx),
        "hermes" => hermes::plan(ctx),
        _ => Err(format!("agent must be one of {}", AGENTS.join(", "))),
    }
}

fn apply(ctx: &Ctx) -> R<String> {
    let Plan {
        mut files,
        native: command,
        after,
    } = plan(ctx)?;
    if let Some(args) = command {
        native(ctx.agent, &args, &ctx.home)?;
    }
    if let Some(after) = after {
        after(&mut files)?;
    }
    for (path, body) in files {
        if read(&path)? == body {
            continue;
        }
        // Removal leaves no empty file behind: one holding only Cairn's part carries nothing.
        if ctx.remove && is_empty_config(&body) {
            // A missing file is already the wanted state; never create an empty one.
            match fs::remove_file(&path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => {
                    return Err(format!("cannot remove {}: {e}", path.display()));
                }
                _ => {}
            }
            // Setup may have created the folders too; `remove_dir` keeps one with anything in it.
            for folder in path.ancestors().skip(1) {
                if !below(&ctx.home, folder) || fs::remove_dir(folder).is_err() {
                    break;
                }
            }
            continue;
        }
        if let Some(parent) = path.parent() {
            create_private_dir(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        write_private(&path, body.as_bytes())
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    let mut done = format!(
        "Cairn {} integration {}",
        ctx.agent,
        if ctx.remove { "removed" } else { "installed" }
    );
    if ctx.agent == "hermes"
        && !ctx.remove
        && let Some(warning) = hermes::version_warning(&ctx.home)
    {
        done.push_str("\ncairn: warning: ");
        done.push_str(&warning);
    }
    if ctx.agent == "hermes"
        && !ctx.remove
        && hermes::gateway_profile(&hermes::config_dir(&ctx.home).join("config.yaml"))
    {
        // Hermes binds chat memory only when the gateway process loads its plugins.
        done.push_str("\nStart the Hermes gateway process again so it loads Cairn chat memory; a running one shows no Cairn tools until then.");
    }
    Ok(done)
}

/// Whether removal may delete `folder`: resolved through links and `..`, it is still below `root`.
pub(super) fn below(root: &Path, folder: &Path) -> bool {
    match (root.canonicalize(), folder.canonicalize()) {
        (Ok(root), Ok(real)) => real != root && real.starts_with(root),
        _ => false,
    }
}

/// Whitespace, or an empty JSON object (also an empty YAML mapping); a comment or any key is content.
pub(super) fn is_empty_config(body: &str) -> bool {
    body.trim().is_empty()
        || serde_json::from_str::<Value>(body)
            .is_ok_and(|v| v.as_object().is_some_and(Map::is_empty))
}

fn configure_with(
    agent: &str,
    remove: bool,
    keep_native_memory: Option<bool>,
    gateway: bool,
    exe: &Path,
    config: &Path,
) -> Result<String, Error> {
    let home = env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| Error::Config("no home directory (set HOME)".to_owned()))?;
    let config = std::path::absolute(config).map_err(|e| Error::Config(e.to_string()))?;
    let exe = exe.to_string_lossy();
    let ctx = Ctx {
        agent,
        remove,
        exe: &exe,
        config: &config,
        gateway,
        keep_native_memory: keep_native_memory.unwrap_or_else(|| {
            agent == "claude"
                && claude::keeps_native_memory(
                    &crate::claude_memory::config_dir(&home).join("settings.json"),
                )
        }),
        home,
    };
    apply(&ctx).map_err(|e| Error::Config(format!("Cairn agent setup failed: {e}")))
}

/// Installs or removes one agent's integration for this executable and the selected config.
pub fn configure(agent: &str, remove: bool) -> Result<String, Error> {
    let exe = env::current_exe().map_err(|e| Error::Config(e.to_string()))?;
    configure_with(agent, remove, None, false, &exe, &selected_config_path()?)
}

/// Installs one agent with the caller's choice about Claude's built-in memory writes.
pub fn install(agent: &str, keep_native_memory: bool, gateway: bool) -> Result<String, Error> {
    let exe = env::current_exe().map_err(|e| Error::Config(e.to_string()))?;
    configure_with(
        agent,
        false,
        Some(keep_native_memory),
        gateway,
        &exe,
        &selected_config_path()?,
    )
}

/// Where each agent keeps the file that proves a Cairn hook is installed.
pub fn hook_files(home: &Path) -> [(&'static str, PathBuf); AGENTS.len()] {
    let dir = |var: &str, default: PathBuf| {
        env::var_os(var)
            .filter(|v| !v.is_empty())
            .map_or(default, PathBuf::from)
    };
    [
        (
            "claude",
            crate::claude_memory::config_dir(home).join("settings.json"),
        ),
        (
            "codex",
            dir("CODEX_HOME", home.join(".codex")).join("hooks.json"),
        ),
        (
            "opencode",
            dir("XDG_CONFIG_HOME", home.join(".config")).join("opencode/plugins/cairn/index.js"),
        ),
        ("grok", grok::hook_file(home)),
        ("antigravity", antigravity::hook_file(home)),
        ("hermes", hermes::hook_file(home)),
    ]
}

/// `cairn update`: rewrites the managed entries of every agent that already has one, so the
/// hook commands follow the new executable. One line per agent that was refreshed or failed.
pub fn refresh(exe: &Path) -> Vec<String> {
    let Some(home) = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
    else {
        return Vec::new();
    };
    let Ok(config) = selected_config_path() else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for (agent, file) in hook_files(&home) {
        if !fs::read_to_string(file).is_ok_and(|text| text.contains(MANAGED_FLAG)) {
            continue;
        }
        lines.push(
            match configure_with(agent, false, None, false, exe, &config) {
                Ok(done) => done,
                Err(error) => format!(
                    "cairn: warning: {agent} hooks were not refreshed ({error}); run `cairn setup`"
                ),
            },
        );
    }
    retire_python(config.parent().unwrap_or(Path::new(".")));
    lines
}

/// Removes what the retired Python hook left in the config directory. A script is deleted only
/// when its checksum is the one `cairn setup` recorded in `hook/installed.json`; an edited one
/// is left with a note. An old version 2 outbox is left alone too (pre-release: not replayed).
pub fn retire_python(config_dir: &Path) {
    let hooks = config_dir.join("hook");
    // Every host, configured now or not: a 0.1.5 run-hook entry fails on each event it fires.
    if let Some(home) = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) {
        for (agent, file) in hook_files(Path::new(&home)) {
            let Ok(before) = read(&file) else { continue };
            if !matches!(agent, "claude" | "codex") || !before.contains("run-hook") {
                continue;
            }
            let written = edit_hooks(&file, &legacy_run_hook, &|_| None).and_then(|body| {
                if body == before {
                    return Ok(());
                }
                write_private(&file, body.as_bytes()).map_err(|e| e.to_string())
            });
            if let Err(error) = written {
                eprintln!(
                    "cairn: warning: old run-hook entries in {} were not removed ({error})",
                    file.display()
                );
            }
        }
    }
    // The wrapper is Cairn's only when it runs the Python hook; a foreign file in a custom config
    // dir stays, and so does the README that goes with it.
    if fs::read_to_string(hooks.join("run-hook")).is_ok_and(|body| body.contains("cairn_hook.py")) {
        for name in ["run-hook", "README.txt"] {
            let _ = fs::remove_file(hooks.join(name));
        }
    }
    let record: Map<String, Value> = fs::read_to_string(hooks.join("installed.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    let mut left = false;
    for name in ["cairn_hook.py", "cairn_outbox.py", "cairn_instructions.txt"] {
        let path = hooks.join(name);
        let Ok(body) = fs::read(&path) else { continue };
        if record.get(name).and_then(Value::as_str)
            == Some(format!("{:x}", Sha256::digest(&body)).as_str())
        {
            let _ = fs::remove_file(&path);
        } else {
            left = true;
            eprintln!(
                "cairn: note: {} is not the copy cairn installed; the old Python hook is retired, delete it yourself",
                path.display()
            );
        }
    }
    if !left {
        let _ = fs::remove_file(hooks.join("installed.json"));
        let _ = fs::remove_dir(&hooks);
    }
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let _ = fs::remove_file(config_dir.join(format!("hook-state.sqlite3{suffix}")));
    }
    let outbox = env::var_os("CAIRN_OUTBOX")
        .filter(|v| !v.is_empty())
        .map_or(config_dir.join("outbox.json"), PathBuf::from);
    let old_queue = fs::read_to_string(&outbox)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .is_some_and(|queue| queue["version"] == 2);
    if old_queue {
        eprintln!(
            "cairn: note: {} is an old Python outbox (version 2); it is left untouched and not replayed",
            outbox.display()
        );
    }
}
