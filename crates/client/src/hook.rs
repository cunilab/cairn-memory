//! `cairn hook <Event> [--agent a] [--config p] [--cairn-managed]`: a stub that runs the
//! installed Python hook until step 4 replaces it. The event itself arrives on stdin.

use crate::{Error, selected_config_path};
use std::path::PathBuf;
use std::process::Command;

/// Returns the hook's exit code. `args` is everything after `hook`.
pub fn run(args: &[String]) -> Result<i32, Error> {
    let (_event, options) = args
        .split_first()
        .ok_or_else(|| Error::Input("hook needs an event name".to_owned()))?;
    let config = match options.iter().position(|arg| arg == "--config") {
        Some(index) => options.get(index + 1).map(PathBuf::from),
        None => None,
    };
    let config = match config {
        Some(path) => path,
        None => selected_config_path()?,
    };
    let script = config.with_file_name("hook").join("cairn_hook.py");
    let status = Command::new("python3")
        .arg(&script)
        .args(options)
        .status()
        .map_err(|error| {
            Error::Input(format!("cannot run python3 {}: {error}", script.display()))
        })?;
    Ok(status.code().unwrap_or(1))
}
