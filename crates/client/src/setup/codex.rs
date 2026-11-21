//! Codex: hook commands in `hooks.json`; the MCP entry goes through `codex mcp add/remove`.

use super::{Ctx, Plan, R, add_args, hook_config, json_config, native, owned, pretty};
use serde_json::{Value, json};

const EVENTS: [(&str, u32); 5] = [
    ("SessionStart", 10),
    ("UserPromptSubmit", 5),
    ("PostToolUse", 10),
    ("Stop", 10),
    ("SessionEnd", 3),
];

pub(super) fn plan(ctx: &Ctx) -> R<Plan> {
    let folder = ctx
        .dir("CODEX_HOME")
        .unwrap_or_else(|| ctx.home.join(".codex"));
    let path = folder.join("hooks.json");
    let mut root = json_config(&path)?;
    hook_config(&mut root, ctx, &EVENTS)?;
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
    Ok(Plan {
        files: vec![(path, pretty(root))],
        native: command,
        after: None,
    })
}
