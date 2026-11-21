//! Claude Code: hook commands in `settings.json`, the MCP entry in `.claude.json`.

use super::{Ctx, Plan, R, hook_config, json_config, pretty, set_mcp};
use serde_json::json;

pub(super) const EVENTS: [(&str, u32); 6] = [
    ("SessionStart", 10),
    ("UserPromptSubmit", 5),
    ("PostToolUse", 10),
    ("Stop", 10),
    ("PreCompact", 10),
    ("SessionEnd", 3),
];

pub(super) fn plan(ctx: &Ctx) -> R<Plan> {
    let folder = ctx.dir("CLAUDE_CONFIG_DIR");
    let settings = folder
        .clone()
        .unwrap_or_else(|| ctx.home.join(".claude"))
        .join("settings.json");
    let mut root = json_config(&settings)?;
    hook_config(&mut root, ctx, &EVENTS)?;
    // The default `~/.claude.json` sits beside `~/.claude`; a custom config dir holds its own.
    let mcp = folder.map_or_else(
        || ctx.home.join(".claude.json"),
        |dir| dir.join(".claude.json"),
    );
    let mut servers = json_config(&mcp)?;
    let entry = json!({"type": "stdio", "command": ctx.exe, "args": ["mcp"], "env": ctx.mcp_env()});
    set_mcp(&mut servers, &["mcpServers"], entry, ctx.remove)?;
    Ok(Plan {
        files: vec![(settings, pretty(root)), (mcp, pretty(servers))],
        native: None,
        after: None,
    })
}
