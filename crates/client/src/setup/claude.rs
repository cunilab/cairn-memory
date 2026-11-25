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
    let settings_text = hooks_text(&settings, ctx, &EVENTS)?;
    let settings_text = if ctx.refresh {
        settings_text
    } else {
        permissions_text(settings_text, ctx.remove)?
    };
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

fn permissions_text(text: String, remove: bool) -> R<String> {
    const RULE: &str = "mcp__cairn";
    const ALLOW: &[&str] = &["permissions", "allow"];
    // The settings schema permits extension keys. Keeping ownership here makes the edit atomic.
    const OWNER: &[&str] = &["cairnManagedMcpAllow"];
    if text.trim().is_empty() {
        return Ok(text);
    }
    let root = jsonc::read(&text)?.value;
    let created = match root.get(OWNER[0]) {
        None if remove => return Ok(text),
        None => None,
        Some(value) => match value.as_str() {
            Some(scope @ ("permissions" | "allow" | "rule")) => Some(scope),
            _ => return Err(format!("invalid {} marker in settings.json", OWNER[0])),
        },
    };
    let permissions = match root.get("permissions") {
        None => None,
        Some(Value::Object(map)) => Some(map),
        Some(_) => return Err("permissions must be an object".into()),
    };
    let allow = match permissions.and_then(|map| map.get("allow")) {
        None => None,
        Some(Value::Array(rules)) => Some(rules),
        Some(_) => return Err("permissions.allow must be an array".into()),
    };
    let rules = allow.map_or(&[][..], Vec::as_slice);
    let index = rules.iter().rposition(|rule| rule == RULE);
    if remove {
        let text = match (index, created) {
            (None, _) => text,
            (Some(_), Some("rule")) if rules.len() == 1 => jsonc::put(&text, ALLOW, &json!([]))?,
            (Some(_), Some("allow"))
                if rules.len() == 1 && permissions.is_some_and(|map| map.len() == 1) =>
            {
                jsonc::put(&text, &["permissions"], &json!({}))?
            }
            (Some(index), _) => {
                jsonc::delete(&text, &["permissions", "allow", &index.to_string()])?
            }
        };
        return jsonc::delete(&text, OWNER);
    }
    if index.is_some() {
        return Ok(text);
    }
    if rules.iter().any(|rule| rule == "mcp__cairn__*") {
        return jsonc::delete(&text, OWNER);
    }
    let created = created.unwrap_or(match (permissions, allow) {
        (None, _) => "permissions",
        (_, None) => "allow",
        _ => "rule",
    });
    let text = jsonc::push(&text, ALLOW, &json!(RULE))?;
    jsonc::put(&text, OWNER, &json!(created))
}
