//! Codex: hook commands in `hooks.json`; the MCP entry goes through `codex mcp add/remove`.

use super::{
    After, Ctx, MARKER, Plan, R, add_args, hooks_text, instruction_file, native, owned, read, which,
};
use serde_json::{Value, json};
use std::path::Path;
use toml_edit::{DocumentMut, Item, value};

const EVENTS: [(&str, u32); 6] = [
    ("SessionStart", 10),
    ("UserPromptSubmit", 5),
    ("PreToolUse", 3),
    ("PostToolUse", 10),
    ("Stop", 10),
    ("SessionEnd", 3),
];

pub(super) fn plan(ctx: &Ctx) -> R<Plan> {
    let folder = ctx
        .dir("CODEX_HOME")
        .unwrap_or_else(|| ctx.home.join(".codex"));
    let path = folder.join("hooks.json");
    let text = hooks_text(&path, ctx, &EVENTS)?;
    let files = vec![
        (path, text),
        instruction_file(folder.join("AGENTS.md"), ctx.remove)?,
    ];
    if ctx.remove && which("codex").is_none() {
        // The CLI is gone: the hook file is still cleaned, its MCP entry is not ours to reach.
        eprintln!("cairn: note: the codex CLI is not installed; its MCP entry was not removed");
        return Ok(Plan {
            files,
            native: None,
            after: None,
        });
    }
    let listed = native("codex", &["list".into(), "--json".into()], &ctx.home)?;
    let entries = match serde_json::from_str::<Value>(&listed) {
        Ok(Value::Array(entries)) => entries,
        _ => return Err("codex mcp list returned an invalid configuration".into()),
    };
    let current = entries.iter().find(|entry| entry["name"] == "cairn");
    let transport = current.map(|entry| &entry["transport"]);
    if transport.is_some_and(|t| !owned(t)) {
        return Err(super::UNMARKED.into());
    }
    let command = if ctx.remove {
        current.map(|_| vec!["remove".to_owned(), "cairn".to_owned()])
    } else if transport.is_some_and(|t| {
        t["command"] == ctx.exe
            && t["args"] == json!(["mcp"])
            && t["env"] == Value::Object(ctx.mcp_env())
    }) {
        None
    } else {
        Some(add_args(vec!["add".into(), "cairn".into()], ctx))
    };
    let after: Option<After> = if ctx.remove {
        None
    } else {
        let config = folder.join("config.toml");
        Some(Box::new(move |files| {
            let text = approve_tools(&config)?;
            files.push((config, text));
            Ok(())
        }))
    };
    Ok(Plan {
        files,
        native: command,
        after,
    })
}

fn approve_tools(path: &Path) -> R<String> {
    const MODE: &str = "default_tools_approval_mode";
    // Native MCP registration has no approval flag. Edit only the table it registered.
    let text = read(path)?;
    let mut config = text
        .parse::<DocumentMut>()
        .map_err(|_| "cannot parse Codex config.toml")?;
    let server = config
        .get_mut("mcp_servers")
        .and_then(|servers| servers.get_mut("cairn"))
        .and_then(Item::as_table_like_mut)
        .ok_or("missing cairn MCP entry in Codex config.toml")?;
    if server
        .get("env")
        .and_then(|env| env.get(MARKER))
        .and_then(Item::as_str)
        != Some("1")
    {
        return Err(super::UNMARKED.into());
    }
    if server.get(MODE).and_then(Item::as_str) == Some("approve") {
        return Ok(text);
    }
    server.insert(MODE, value("approve"));
    Ok(config.to_string())
}
