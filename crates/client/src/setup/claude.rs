//! Claude Code: hook commands in `settings.json`, the MCP entry in `.claude.json`.

use super::{Ctx, Plan, R, hooks_text, mcp_text};
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
    // The default `~/.claude.json` sits beside `~/.claude`; a custom config dir holds its own.
    let mcp = folder.map_or_else(
        || ctx.home.join(".claude.json"),
        |dir| dir.join(".claude.json"),
    );
    let entry = json!({"type": "stdio", "command": ctx.exe, "args": ["mcp"], "env": ctx.mcp_env()});
    Ok(Plan {
        files: vec![
            (settings.clone(), hooks_text(&settings, ctx, &EVENTS)?),
            (mcp.clone(), mcp_text(&mcp, &entry, ctx.remove)?),
        ],
        native: None,
        after: None,
    })
}
