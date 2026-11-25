//! Antigravity CLI shares its MCP, hooks and global rules with the Antigravity App.

use super::{
    APPROVAL_MARKER, Ctx, MANAGED_FLAG, Plan, R, edit_json, instruction_file, jsonc, mcp_text,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const RULE: &str = "mcp(cairn/*)";

pub(crate) fn config_dir(home: &Path) -> PathBuf {
    home.join(".gemini/config")
}

pub(crate) fn hook_file(home: &Path) -> PathBuf {
    config_dir(home).join("hooks.json")
}

pub(super) fn plan(ctx: &Ctx) -> R<Plan> {
    let folder = config_dir(&ctx.home);
    let mcp = folder.join("mcp_config.json");
    let mut env = ctx.mcp_env();
    let mut owned_approval = false;
    edit_json(&mcp, |text, root| {
        owned_approval = root
            .get("mcpServers")
            .and_then(|servers| servers.get("cairn"))
            .is_some_and(|entry| super::owned(entry) && entry["env"][APPROVAL_MARKER] == "1");
        Ok(text.to_owned())
    })?;
    let settings = ctx.home.join(".gemini/antigravity-cli/settings.json");
    let (settings_text, added) = permissions_text(&settings, ctx.remove, owned_approval)?;
    if added || owned_approval {
        env.insert(APPROVAL_MARKER.into(), json!("1"));
    }
    let entry = json!({"command": ctx.exe, "args": ["mcp"], "env": env});
    let mut files = vec![
        (mcp.clone(), mcp_text(&mcp, &entry, ctx.remove)?),
        (settings, settings_text),
    ];
    if ctx.remove {
        files.reverse();
    }
    let hooks = hook_file(&ctx.home);
    files.push((hooks.clone(), hooks_text(&hooks, ctx)?));
    files.push(instruction_file(folder.join("AGENTS.md"), ctx.remove)?);
    Ok(Plan {
        files,
        native: None,
        after: None,
    })
}

fn permissions_text(path: &Path, remove: bool, owned: bool) -> R<(String, bool)> {
    const ALLOW: &[&str] = &["permissions", "allow"];
    let mut added = false;
    let text = edit_json(path, |text, root| {
        let rules = match root.get("permissions") {
            None => None,
            Some(Value::Object(map)) => map.get("allow"),
            Some(_) => return Err("permissions must be an object".into()),
        };
        let index = match rules {
            None => None,
            Some(Value::Array(rules)) => rules.iter().position(|rule| rule == RULE),
            Some(_) => return Err("permissions.allow must be an array".into()),
        };
        match (index, remove, owned) {
            (Some(index), true, true) => {
                jsonc::delete(text, &["permissions", "allow", &index.to_string()])
            }
            (None, false, _) => {
                added = true;
                jsonc::push(text, ALLOW, &json!(RULE))
            }
            _ => Ok(text.to_owned()),
        }
    })?;
    Ok((text, added))
}

fn hooks_text(path: &Path, ctx: &Ctx) -> R<String> {
    edit_json(path, |text, root| {
        if root.get("cairn").is_some_and(|group| {
            !group["SessionStart"][0]["command"]
                .as_str()
                .is_some_and(|command| command.ends_with(MANAGED_FLAG))
        }) {
            return Err(
                "an unmarked cairn hook group exists; rename or remove it explicitly".into(),
            );
        }
        if ctx.remove {
            return jsonc::delete(text, &["cairn"]);
        }
        let handler = |event, timeout| json!({"type": "command", "command": ctx.hook_command(event), "timeout": timeout});
        jsonc::put(
            text,
            &["cairn"],
            &json!({
                "SessionStart": [handler("SessionStart", 10)],
                "PreInvocation": [handler("PreInvocation", 5)],
                "PreToolUse": [{
                    "matcher": "^run_command$",
                    "hooks": [handler("PreToolUse", 3)]
                }],
                "PostToolUse": [{
                    "matcher": "^(run_command|write_to_file|replace_file_content|multi_replace_file_content|mcp_cairn_.*)$",
                    "hooks": [handler("PostToolUse", 10)]
                }],
                "Stop": [handler("Stop", 10)]
            }),
        )
    })
}
