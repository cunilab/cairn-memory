//! Apply only the closed set of device actions from the server.
use crate::{Client, Config, Error, project, setup, update};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    SuggestSetup {
        device_id: String,
    },
    RunUpdate {
        target: String,
    },
    WriteProjectFile {
        project_id: String,
        repository: String,
        name: String,
    },
}

/// Execute one action. The update callback allows tests to avoid network and executable changes.
pub fn execute_action(
    action: Value,
    cwd: &Path,
    updater: impl FnOnce() -> Result<(), Error>,
) -> Result<&'static str, &'static str> {
    let action: Action = serde_json::from_value(action).map_err(|_| "failed")?;
    match action {
        Action::RunUpdate { target } => {
            if !cairn_core::account::version_older(update::VERSION, &target) {
                return Err("already_current");
            }
            updater().map_err(|_| "failed")?;
            Ok("updated Cairn")
        }
        Action::WriteProjectFile {
            project_id,
            repository,
            name,
        } => {
            let top = project::toplevel(cwd)
                .ok_or("not_this_repo")?
                .canonicalize()
                .map_err(|_| "failed")?;
            if project::repository_name(&top) != repository {
                return Err("not_this_repo");
            }
            let path = top.join(".cairn/project.json");
            if path.symlink_metadata().is_ok() {
                return Err("file_exists");
            }
            project::write_new(&top, &project_id, &repository, &name).map_err(
                |error| match error.kind() {
                    std::io::ErrorKind::AlreadyExists => "file_exists",
                    std::io::ErrorKind::PermissionDenied => "outside_checkout",
                    _ => "failed",
                },
            )?;
            Ok("wrote .cairn/project.json")
        }
        Action::SuggestSetup { device_id } => {
            if device_id.is_empty() {
                return Err("failed");
            }
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .ok_or("failed")?;
            for (agent, path) in setup::hook_files(Path::new(&home)) {
                if std::fs::read_to_string(path).is_ok_and(|text| text.contains("--cairn-managed"))
                {
                    setup::configure(agent, false).map_err(|_| "failed")?;
                }
            }
            Ok("refreshed agent setup")
        }
    }
}

fn fetch(client: &Client) -> Result<Vec<Value>, Error> {
    serde_json::from_value(client.get("/cairn-memory/device/fixes")?)
        .map_err(|error| Error::Input(format!("invalid fixes response: {error}")))
}

pub fn list() -> Result<(), Error> {
    let client = Client::new(Config::load()?);
    for fix in fetch(&client)? {
        println!("{}", fix);
    }
    Ok(())
}

pub fn apply(id: Option<&str>) -> Result<(), Error> {
    apply_with_timeout(id, None)
}

pub fn apply_automatically() -> Result<(), Error> {
    apply_with_timeout(None, Some(std::time::Duration::from_secs(3)))
}

fn apply_with_timeout(id: Option<&str>, timeout: Option<std::time::Duration>) -> Result<(), Error> {
    let config = if timeout.is_some() {
        Config::load_from(&crate::selected_config_path()?)?
    } else {
        Config::load()?
    };
    let fetch_client = timeout.map(|timeout| Client::with_timeout(config.clone(), timeout));
    let client = Client::new(config);
    let cwd = std::env::current_dir().map_err(|error| Error::Input(error.to_string()))?;
    for fix in fetch(fetch_client.as_ref().unwrap_or(&client))? {
        let Some(fix_id) = fix.get("fix_id").and_then(Value::as_str) else {
            continue;
        };
        if id.is_some_and(|id| id != fix_id) {
            continue;
        }
        if !fix_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            continue;
        }
        let result = execute_action(
            fix.get("action").cloned().unwrap_or(Value::Null),
            &cwd,
            || update::update(false),
        );
        let body = match result {
            Ok(_) => json!({"result":"applied"}),
            Err(reason) => json!({"result":"skipped", "reason":reason}),
        };
        client.post(&format!("/cairn-memory/device/fixes/{fix_id}"), &body)?;
        match result {
            Ok(message) => println!("Applied {fix_id}: {message}"),
            Err(reason) => println!("Skipped {fix_id}: {reason}"),
        }
    }
    Ok(())
}
