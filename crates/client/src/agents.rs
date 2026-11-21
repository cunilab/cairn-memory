use crate::{Error, selected_config_path};
use serde_json::json;
use std::io::Write;
use std::process::{Command, Stdio};

pub const AGENTS: [&str; 3] = ["claude", "codex", "opencode"];

/// Python supplies portable private file replacement and delegates MCP config to agent CLIs.
pub fn configure(agent: &str, remove: bool) -> Result<String, Error> {
    let exe = std::env::current_exe().map_err(|e| Error::Config(e.to_string()))?;
    let config =
        std::path::absolute(selected_config_path()?).map_err(|e| Error::Config(e.to_string()))?;
    let spec = json!({"agent":agent, "remove":remove, "config":config, "exe":exe, "instructions":crate::tools::instructions()});
    let mut child = Command::new("python3")
        .args(["-c", include_str!("../../../scripts/cairn_setup.py")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Config(format!("python3 is required for agent setup: {e}")))?;
    child
        .stdin
        .take()
        .ok_or_else(|| Error::Config("cannot open setup input".into()))?
        .write_all(spec.to_string().as_bytes())
        .map_err(|e| Error::Config(e.to_string()))?;
    let output = child
        .wait_with_output()
        .map_err(|e| Error::Config(e.to_string()))?;
    if !output.status.success() {
        return Err(Error::Config(
            String::from_utf8_lossy(&output.stderr).trim().into(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}
