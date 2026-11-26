//! Grok Build stores personal MCP and permissions in TOML and loads separate JSON hook files.

use super::{
    APPROVAL_MARKER, Ctx, MANAGED_FLAG, MARKER, Plan, R, UNMARKED, hooks_text, instruction_file,
    read,
};
use std::path::{Path, PathBuf};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value, value};

const EVENTS: [(&str, u32); 7] = [
    ("SessionStart", 10),
    ("UserPromptSubmit", 5),
    ("PreToolUse", 3),
    ("PostToolUse", 10),
    ("Stop", 10),
    ("PreCompact", 10),
    ("SessionEnd", 3),
];
const RULE: &str = "MCPTool(cairn__*)";

pub(crate) fn config_dir(home: &Path) -> PathBuf {
    std::env::var_os("GROK_HOME")
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| home.join(".grok"), PathBuf::from)
}

pub(crate) fn hook_file(home: &Path) -> PathBuf {
    config_dir(home).join("hooks/cairn.json")
}

pub(crate) fn hooks_installed(home: &Path) -> bool {
    read(&hook_file(home)).is_ok_and(|text| text.contains(MANAGED_FLAG))
}

pub(super) fn plan(ctx: &Ctx) -> R<Plan> {
    let folder = config_dir(&ctx.home);
    let config = folder.join("config.toml");
    let hooks = hook_file(&ctx.home);
    Ok(Plan {
        files: vec![
            (config.clone(), config_text(&config, ctx)?),
            (
                hooks.clone(),
                cairn_hooks(hooks_text(&hooks, ctx, &EVENTS)?, ctx.remove),
            ),
            instruction_file(folder.join("AGENTS.md"), ctx.remove)?,
        ],
        native: None,
        after: None,
    })
}

/// Cairn owns `hooks/cairn.json`: removal deletes it once no hooks remain in it.
fn cairn_hooks(text: String, remove: bool) -> String {
    let empty = serde_json::from_str::<serde_json::Value>(&text)
        .is_ok_and(|value| value.as_object().is_some_and(serde_json::Map::is_empty));
    if remove && empty { String::new() } else { text }
}

fn config_text(path: &Path, ctx: &Ctx) -> R<String> {
    let mut doc = read(path)?
        .parse::<DocumentMut>()
        .map_err(|_| "cannot parse Grok config.toml")?;
    let existing = doc
        .get("mcp_servers")
        .and_then(|servers| servers.get("cairn"));
    let marked = |entry: &Item, key| {
        entry
            .get("env")
            .and_then(|env| env.get(key))
            .and_then(Item::as_str)
            == Some("1")
    };
    if existing.is_some_and(|entry| !marked(entry, MARKER)) {
        return Err(UNMARKED.into());
    }
    let managed = existing.is_some_and(|entry| marked(entry, APPROVAL_MARKER));
    let managed = permissions(&mut doc, ctx.remove, managed)?;
    if doc.get("mcp_servers").is_none() && !ctx.remove {
        let mut servers = Table::new();
        servers.set_implicit(true);
        doc["mcp_servers"] = Item::Table(servers);
    }
    if let Some(servers) = doc.get_mut("mcp_servers") {
        let servers = servers
            .as_table_like_mut()
            .ok_or("mcp_servers must be a table")?;
        if ctx.remove {
            servers.remove("cairn");
        } else {
            let mut entry = Table::new();
            entry.insert("command", value(ctx.exe));
            entry.insert("args", value(["mcp"].into_iter().collect::<Array>()));
            let mut env = InlineTable::new();
            let mut expected_env = ctx.mcp_env();
            if managed {
                expected_env.insert(APPROVAL_MARKER.into(), serde_json::json!("1"));
            }
            for (key, val) in &expected_env {
                env.insert(key, Value::from(val.as_str().unwrap_or_default()));
            }
            entry.insert("env", value(env));
            let matches = servers.get("cairn").is_some_and(|server| {
                server.get("command").and_then(Item::as_str) == Some(ctx.exe)
                    && server
                        .get("args")
                        .and_then(Item::as_array)
                        .is_some_and(|args| {
                            args.len() == 1 && args.get(0).and_then(Value::as_str) == Some("mcp")
                        })
                    && server
                        .get("env")
                        .and_then(Item::as_table_like)
                        .is_some_and(|env| {
                            env.len() == expected_env.len()
                                && expected_env.iter().all(|(key, val)| {
                                    env.get(key).and_then(Item::as_str) == val.as_str()
                                })
                        })
            });
            if !matches {
                servers.insert("cairn", Item::Table(entry));
            }
        }
    }
    Ok(doc.to_string())
}

fn permissions(doc: &mut DocumentMut, remove: bool, mut managed: bool) -> R<bool> {
    if doc.get("permission").is_none() && !remove {
        doc["permission"] = Item::Table(Table::new());
    }
    if let Some(permission) = doc.get_mut("permission") {
        let permission = permission
            .as_table_like_mut()
            .ok_or("permission must be a table")?;
        if !remove && permission.get("allow").is_none() {
            permission.insert("allow", value(Array::new()));
        }
        if let Some(allow) = permission.get_mut("allow") {
            let allow = allow
                .as_array_mut()
                .ok_or("permission.allow must be an array")?;
            let index = allow.iter().position(|item| item.as_str() == Some(RULE));
            match (index, remove, managed) {
                (Some(index), true, true) => {
                    allow.remove(index);
                    if allow.is_empty() {
                        permission.remove("allow");
                        if permission.is_empty() {
                            doc.remove("permission");
                        }
                    }
                }
                (None, false, _) => {
                    allow.push(RULE);
                    managed = true;
                }
                _ => {}
            }
        }
    }
    Ok(managed)
}
