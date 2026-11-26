//! OpenCode: `AGENTS.md` block, the plugin that runs `cairn hook`, and the MCP entry through
//! `opencode mcp add`. Removal edits `opencode.json[c]` by token span, keeping its comments.

use super::{Ctx, Plan, R, add_args, instruction_file, jsonc, owned, read, require_cli};
use serde_json::json;
use std::fs;
use std::path::PathBuf;

/// Starts the plugin file, so only a file Cairn wrote is ever replaced or deleted.
const MARKER: &str = "// cairn:continuity\n";
const PLUGIN: &str = include_str!("plugin.js");

fn render(exe: &str, config: &str) -> String {
    let quoted = |text: &str| serde_json::to_string(text).expect("a string serializes");
    PLUGIN
        .replace("__CAIRN_EXE__", &quoted(exe))
        .replace("__CAIRN_SETUP_CONFIG__", &quoted(config))
}

pub(super) fn plan(ctx: &Ctx) -> R<Plan> {
    // Removal edits files only, so it works after the `opencode` CLI is uninstalled.
    if !ctx.remove {
        require_cli("opencode")?;
    }
    let xdg = ctx
        .dir("XDG_CONFIG_HOME")
        .unwrap_or_else(|| ctx.home.join(".config"));
    let base = xdg.join("opencode");
    let choices: Vec<PathBuf> = ["opencode.json", "opencode.jsonc"]
        .iter()
        .map(|name| base.join(name))
        .filter(|p| p.exists())
        .collect();
    if choices.len() > 1 {
        return Err(
            "both opencode.json and opencode.jsonc exist; choose one global config first".into(),
        );
    }
    let path = choices
        .into_iter()
        .next()
        .unwrap_or_else(|| base.join("opencode.json"));
    let raw = read(&path)?;
    let root = jsonc::read(if raw.is_empty() { "{}" } else { &raw })?;
    let mcp = root.value.get("mcp").cloned().unwrap_or_else(|| json!({}));
    let servers = mcp.get("servers").cloned().unwrap_or_else(|| json!({}));
    if !mcp.is_object() || !servers.is_object() {
        return Err("OpenCode V2 requires an mcp.servers object".into());
    }
    if mcp.get("cairn").is_some() {
        return Err("OpenCode V1 MCP config found; migrate to V2 first".into());
    }
    let entry = servers.get("cairn");
    if entry.is_some_and(|e| !owned(e)) {
        return Err(super::UNMARKED.into());
    }
    let desired =
        json!({"type": "local", "command": [ctx.exe, "mcp"], "environment": ctx.mcp_env()});
    let mut files = Vec::new();
    let mut native = None;
    if ctx.remove {
        let allow_trailing = path.extension().is_some_and(|ext| ext == "jsonc");
        files.push((
            path.clone(),
            jsonc::remove_cairn(&raw, &root, allow_trailing)?,
        ));
    } else if entry != Some(&desired) {
        native = Some(add_args(
            vec!["add".into(), "cairn".into(), "--global".into()],
            ctx,
        ));
    }
    files.push(instruction_file(base.join("AGENTS.md"), ctx.remove)?);
    let (remove, rendered) = (ctx.remove, render(ctx.exe, &ctx.config.to_string_lossy()));
    let after: super::After = Box::new(move |files| {
        // The native command may have rewritten the file: read what is there now.
        let current = files
            .iter()
            .find(|f| f.0 == path)
            .map_or_else(|| read(&path), |f| Ok(f.1.clone()))?;
        let current = if current.is_empty() {
            "{}".to_owned()
        } else {
            current
        };
        let edited = jsonc::remove_retired_plugins(&current, &jsonc::read(&current)?)?;
        if edited != read(&path)? {
            files.retain(|f| f.0 != path);
            files.push((path.clone(), edited));
        }
        let ours = |file: &PathBuf| read(file).is_ok_and(|text| text.starts_with(MARKER));
        let legacy = base.join("plugin/cairn.js");
        if legacy.is_file() && ours(&legacy) {
            let _ = fs::remove_file(&legacy);
        }
        let index = base.join("plugins/cairn/index.js");
        if remove {
            if index.is_file() && ours(&index) {
                let _ = fs::remove_file(&index);
            }
            let _ = fs::remove_dir(base.join("plugins/cairn"));
            let _ = fs::remove_dir(base.join("plugins"));
        } else if !index.exists() || ours(&index) {
            files.push((index, rendered));
        }
        Ok(())
    });
    Ok(Plan {
        files,
        native,
        after: Some(after),
    })
}
