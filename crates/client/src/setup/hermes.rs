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
    let (text, gateway) = config_text(&config, ctx)?;
    let config = (config, text);
    let instructions = instruction_file(folder.join("SOUL.md"), ctx.remove || gateway)?;
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
    let tools = crate::tools::tools_list()["tools"].to_string();
    let script = PLUGIN
        .replacen("__CAIRN_PATHS__", &paths, 1)
        .replacen(
            "__CAIRN_GATEWAY__",
            if gateway { "True" } else { "False" },
            1,
        )
        .replacen("__CAIRN_TOOLS__", &json!(tools).to_string(), 1);
    Ok(Plan {
        files: vec![
            config,
            (hook, script),
            (manifest, MANIFEST.to_owned()),
            instructions,
        ],
        native: None,
        after: None,
    })
}

fn parse_config(raw: &str) -> R<Value> {
    let mut root: Value =
        yaml_serde::from_str(raw).map_err(|_| "cannot parse Hermes config.yaml")?;
    root.apply_merge()
        .map_err(|_| "cannot resolve Hermes config.yaml merges")?;
    Ok(root)
}

fn plugin_settings(root: &Value) -> &Value {
    &root["plugins"]["entries"][PLUGIN_NAME]["settings"]
}

/// Whether the profile selects Cairn as its Hermes memory provider.
pub(crate) fn provider_set(path: &Path) -> bool {
    read(path)
        .and_then(|raw| parse_config(&raw))
        .is_ok_and(|root| root["memory"]["provider"].as_str() == Some(PLUGIN_NAME))
}

pub(crate) fn gateway_profile(path: &Path) -> bool {
    read(path)
        .and_then(|raw| parse_config(&raw))
        .is_ok_and(|root| plugin_settings(&root)["cairn_gateway"].as_bool() == Some(true))
}

/// `[]` when `memory.provider` was absent, `[value]` with its exact value otherwise.
const PREVIOUS_PROVIDER: &str = "previous_memory_provider";

fn config_text(path: &Path, ctx: &Ctx) -> R<(String, bool)> {
    let raw = read(path)?;
    let mut root = parse_config(&raw)?;
    let before = root.clone();
    let settings = plugin_settings(&root);
    let had_settings = settings.is_mapping();
    let gateway = !ctx.remove && (ctx.gateway || settings["cairn_gateway"].as_bool() == Some(true));
    let saved = settings.get(PREVIOUS_PROVIDER).cloned();
    let mut owns_enable = settings[ENABLED_MARKER].as_bool() == Some(true);
    let config = as_mapping(&mut root, "Hermes config.yaml")?;

    // Cairn's provider tools replace the MCP entry in Hermes; earlier setups left one.
    let servers = as_mapping(
        config.entry("mcp_servers".into()).or_insert(Value::Null),
        "mcp_servers",
    )?;
    if let Some(existing) = servers.get("cairn") {
        let marked = |marker| existing["env"][marker].as_str() == Some("1");
        if !marked(MARKER) {
            return Err(UNMARKED.into());
        }
        owns_enable |= marked(ENABLED_MARKER);
        servers.remove("cairn");
    }
    if servers.is_empty() {
        config.remove("mcp_servers");
    }

    let memory = as_mapping(
        config.entry("memory".into()).or_insert(Value::Null),
        "memory",
    )?;
    let current = memory.get("provider").cloned();
    let ours = current.as_ref().and_then(Value::as_str) == Some(PLUGIN_NAME);
    let mut previous = None;
    if ctx.remove {
        // Restore only while Cairn still holds the slot: a later user choice wins.
        if ours {
            match saved
                .as_ref()
                .and_then(Value::as_sequence)
                .and_then(|v| v.first())
            {
                Some(value) => memory.insert("provider".into(), value.clone()),
                None => memory.remove("provider"),
            };
        }
    } else if !ours {
        previous = Some(Value::Sequence(current.into_iter().collect()));
        memory.insert("provider".into(), PLUGIN_NAME.into());
    }
    if memory.is_empty() {
        config.remove("memory");
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

    if !ctx.remove || had_settings {
        let settings = settings_mut(config)?;
        for key in ["chat_owner_hashes", "chat_projects"] {
            settings.remove(key);
        }
        if ctx.remove {
            // The chat salt stays so a later setup finds the profile's chat memories.
            for key in ["cairn_gateway", ENABLED_MARKER, PREVIOUS_PROVIDER] {
                settings.remove(key);
            }
        } else {
            if owns_enable {
                settings.insert(ENABLED_MARKER.into(), true.into());
            }
            if let Some(previous) = previous {
                settings.insert(PREVIOUS_PROVIDER.into(), previous);
            }
            if gateway {
                settings.insert("cairn_gateway".into(), true.into());
                settings
                    .entry("chat_salt".into())
                    .or_insert_with(|| cairn_core::id::new_id("salt").into());
            }
        }
        prune_settings(config);
    }
    if root == before {
        return Ok((raw, gateway));
    }
    let text = yaml_serde::to_string(&root).map_err(|_| "cannot serialize Hermes config.yaml")?;
    Ok((text, gateway))
}

fn settings_mut(config: &mut Mapping) -> R<&mut Mapping> {
    let mut map = config;
    for (key, name) in [
        ("plugins", "plugins"),
        ("entries", "plugins.entries"),
        (PLUGIN_NAME, PLUGIN_NAME),
        ("settings", "Cairn plugins settings"),
    ] {
        map = as_mapping(map.entry(key.into()).or_insert(Value::Null), name)?;
    }
    Ok(map)
}

/// Drops the settings path's mappings that this run left empty, innermost first.
fn prune_settings(config: &mut Mapping) {
    fn prune(map: &mut Mapping, path: &[&str]) {
        let Some((key, rest)) = path.split_first() else {
            return;
        };
        if let Some(child) = map.get_mut(*key).and_then(Value::as_mapping_mut) {
            prune(child, rest);
            if child.is_empty() {
                map.remove(*key);
            }
        }
    }
    prune(config, &["plugins", "entries", PLUGIN_NAME, "settings"]);
}

fn as_mapping<'a>(value: &'a mut Value, name: &str) -> R<&'a mut Mapping> {
    if value.is_null() {
        *value = Value::Mapping(Mapping::new());
    }
    value
        .as_mapping_mut()
        .ok_or_else(|| format!("{name} must be a mapping"))
}
