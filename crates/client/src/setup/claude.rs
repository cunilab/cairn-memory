//! Claude Code: hooks and MCP permissions in `settings.json`, the MCP entry in `.claude.json`.

use super::{Ctx, Plan, R, hooks_text, jsonc, mcp_text};
use serde_json::{Value, json};

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
    let settings_text = permissions_text(hooks_text(&settings, ctx, &EVENTS)?, ctx.remove)?;
    let mut files = vec![
        (settings, settings_text),
        (mcp.clone(), mcp_text(&mcp, &entry, ctx.remove)?),
    ];
    // Register the server before granting its permission and revoke permission before removal.
    if !ctx.remove {
        files.reverse();
    }
    Ok(Plan {
        files,
        native: None,
        after: None,
    })
}

fn permissions_text(mut text: String, remove: bool) -> R<String> {
    const RULE: &str = "mcp__cairn";
    const ALLOW: &[&str] = &["permissions", "allow"];
    if text.trim().is_empty() {
        return Ok(text);
    }
    let root = jsonc::read(&text)?.value;
    let allow = match root.get("permissions") {
        None => None,
        Some(Value::Object(map)) => map.get("allow"),
        Some(_) => return Err("permissions must be an object".into()),
    };
    let rules = match allow {
        None => &[][..],
        Some(Value::Array(rules)) => rules,
        Some(_) => return Err("permissions.allow must be an array".into()),
    };
    if remove {
        for (index, rule) in rules.iter().enumerate().rev() {
            if rule == RULE {
                text = jsonc::delete_item(&text, &["permissions", "allow", &index.to_string()])?;
            }
        }
    } else if !rules
        .iter()
        .any(|rule| rule == RULE || rule == "mcp__cairn__*")
    {
        return jsonc::push(&text, ALLOW, &json!(RULE));
    }
    Ok(text)
}
