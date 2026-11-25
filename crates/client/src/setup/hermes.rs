//! Hermes loads scoped MCP servers from YAML and native plugins from its profile directory.

use super::{Ctx, MARKER, Plan, R, UNMARKED, instruction_file, read};
use serde_json::json;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::{env, fs};
use yaml_serde::{Mapping, Value};

const PLUGIN_NAME: &str = "cairn-memory";
const ENABLED_MARKER: &str = "CAIRN_SETUP_PLUGIN_ENABLED";
const FILE_MARKER: &str = "# CAIRN_SETUP_MANAGED\n";
const MANIFEST: &str = include_str!("hermes/plugin.yaml");
const PLUGIN: &str = include_str!("hermes/plugin.py");

pub(crate) fn config_dir(home: &Path) -> PathBuf {
    if let Ok(dir) = env::var("HERMES_HOME")
        && !dir.trim().is_empty()
    {
        let dir = dir.trim();
        return dir
            .strip_prefix("~/")
            .map_or_else(|| PathBuf::from(dir), |relative| home.join(relative));
    }
    let suffix = env::var("HERMES_DATA_DIR_SUFFIX").unwrap_or_default();
    if cfg!(windows) {
        let local = env::var_os("LOCALAPPDATA")
            .filter(|dir| !dir.is_empty())
            .map_or_else(|| home.join("AppData/Local"), PathBuf::from);
        return local.join(format!("hermes{suffix}"));
    }
    home.join(format!(".hermes{suffix}"))
}

pub(crate) fn hook_file(home: &Path) -> PathBuf {
    config_dir(home)
        .join("plugins")
        .join(PLUGIN_NAME)
        .join("__init__.py")
}

pub(super) fn plan(ctx: &Ctx) -> R<Plan> {
    let folder = config_dir(&ctx.home);
    let hook = hook_file(&ctx.home);
    let manifest = hook.with_file_name("plugin.yaml");
    for path in [&hook, &manifest] {
        let before = read(path)?;
        if !before.starts_with(FILE_MARKER) && path.symlink_metadata().is_ok() {
            return Err("an unmarked Hermes cairn-memory plugin exists; move it first".into());
        }
    }
    if !ctx.remove
        && ["plugin.yml", "plugin.json"]
            .iter()
            .any(|name| hook.with_file_name(name).exists())
    {
        return Err("an unmarked Hermes cairn-memory plugin exists; move it first".into());
    }
    let config = folder.join("config.yaml");
    let config = (config.clone(), config_text(&config, ctx)?);
    let instructions = instruction_file(folder.join("SOUL.md"), ctx.remove)?;
    if ctx.remove {
        let after: super::After = Box::new(move |_| {
            for path in [&manifest, &hook] {
                if read(path)?.starts_with(FILE_MARKER) {
                    fs::remove_file(path)
                        .or_else(|error| match error.kind() {
                            ErrorKind::NotFound => Ok(()),
                            _ => Err(error),
                        })
                        .map_err(|error| format!("cannot remove {}: {error}", path.display()))?;
                }
            }
            Ok(())
        });
        return Ok(Plan {
            files: vec![config, instructions],
            native: None,
            after: Some(after),
        });
    }
    let paths = json!([ctx.exe, ctx.config.to_string_lossy()]).to_string();
    let script = PLUGIN.replacen("__CAIRN_PATHS__", &paths, 1);
    Ok(Plan {
        files: vec![
            (hook, script),
            (manifest, MANIFEST.into()),
            config,
            instructions,
        ],
        native: None,
        after: None,
    })
}

fn config_text(path: &Path, ctx: &Ctx) -> R<String> {
    let raw = read(path)?;
    let mut root: Value =
        yaml_serde::from_str(&raw).map_err(|_| "cannot parse Hermes config.yaml")?;
    root.apply_merge()
        .map_err(|_| "cannot resolve Hermes config.yaml merges")?;
    let before = root.clone();
    let config = as_mapping(&mut root, "Hermes config.yaml")?;
    let servers = as_mapping(
        config.entry("mcp_servers".into()).or_insert(Value::Null),
        "mcp_servers",
    )?;
    let existing = servers.get("cairn");
    let marked = |marker| existing.and_then(|server| server["env"][marker].as_str()) == Some("1");
    if existing.is_some() && !marked(MARKER) {
        return Err(UNMARKED.into());
    }
    let mut owns_enable = marked(ENABLED_MARKER);
    if ctx.remove {
        if servers.remove("cairn").is_none() {
            return Ok(raw);
        }
        if servers.is_empty() {
            config.remove("mcp_servers");
        }
    }

    if !ctx.remove || owns_enable {
        let plugins = as_mapping(
            config.entry("plugins".into()).or_insert(Value::Null),
            "plugins",
        )?;
        let name = Value::from(PLUGIN_NAME);
        if !ctx.remove
            && plugins
                .get("disabled")
                .and_then(Value::as_sequence)
                .is_some_and(|list| list.contains(&name))
        {
            return Err("cairn-memory is disabled in Hermes plugins.disabled".into());
        }
        let enabled = plugins.entry("enabled".into()).or_insert(Value::Null);
        if enabled.is_null() {
            *enabled = Value::Sequence(Vec::new());
        }
        let enabled = enabled
            .as_sequence_mut()
            .ok_or("plugins.enabled must be a list")?;
        if ctx.remove {
            enabled.retain(|entry| entry != &name);
        } else if !enabled.contains(&name) {
            enabled.push(name);
            owns_enable = true;
        }
    }

    if !ctx.remove {
        let mut env = ctx.mcp_env();
        // Hermes filters the inherited stdio environment, including CAIRN_CONFIG.
        env.insert("CAIRN_CONFIG".into(), json!(ctx.config.to_string_lossy()));
        if owns_enable {
            env.insert(ENABLED_MARKER.into(), json!("1"));
        }
        let entry = json!({"command": ctx.exe, "args": ["mcp"], "env": env, "trust": "full"});
        as_mapping(
            config.entry("mcp_servers".into()).or_insert(Value::Null),
            "mcp_servers",
        )?
        .insert(
            "cairn".into(),
            yaml_serde::to_value(entry).map_err(|_| "cannot serialize Hermes MCP entry")?,
        );
    }
    if root == before {
        return Ok(raw);
    }
    yaml_serde::to_string(&root).map_err(|_| "cannot serialize Hermes config.yaml".into())
}

fn as_mapping<'a>(value: &'a mut Value, name: &str) -> R<&'a mut Mapping> {
    if value.is_null() {
        *value = Value::Mapping(Mapping::new());
    }
    value
        .as_mapping_mut()
        .ok_or_else(|| format!("{name} must be a mapping"))
}
