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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_an_event_name() {
        assert!(run(&[]).is_err());
    }

    #[test]
    fn runs_the_python_hook_next_to_the_config_and_returns_its_exit_code() {
        let dir = std::env::temp_dir().join(format!("cairn-hook-stub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("hook")).unwrap();
        std::fs::write(
            dir.join("hook/cairn_hook.py"),
            "import sys\nsys.exit(7 if '--cairn-managed' in sys.argv[1:] and 'Stop' not in sys.argv else 1)\n",
        )
        .unwrap();
        let config = dir.join("config.json").to_string_lossy().into_owned();
        let args = ["Stop", "--config", config.as_str(), "--cairn-managed"].map(str::to_owned);
        assert_eq!(run(&args).unwrap(), 7);
        // The event is not an argument of the script; options only.
        let args = ["Start", "--config", config.as_str()].map(str::to_owned);
        assert_eq!(run(&args).unwrap(), 1);
    }
}
