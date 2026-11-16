//! The committed `.cairn/project.json`: which Cairn project a repository belongs to. It holds
//! the project id, repository and display name, no secret and no server URL. A capture that carries
//! the id makes the capturer a member of the project.

use crate::Error;
use serde::Deserialize;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const FILE: &str = ".cairn/project.json";

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (output.status.success() && !text.is_empty()).then(|| text.to_owned())
}

/// The top-level directory of the git checkout that contains `dir`.
pub fn toplevel(dir: &Path) -> Option<PathBuf> {
    git(dir, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

/// Stable remote identity, with a marked directory fallback for repositories without a remote.
pub fn repository_name(top: &Path) -> String {
    git(top, &["config", "--get", "remote.origin.url"])
        .and_then(|origin| normalize_remote(&origin))
        .unwrap_or_else(|| {
            format!(
                "local:{}",
                top.file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default()
            )
        })
}

fn normalize_remote(remote: &str) -> Option<String> {
    let remote = remote.trim().trim_end_matches('/').trim_end_matches(".git");
    let (host, path) = if let Some((scheme, rest)) = remote.split_once("://") {
        if !matches!(scheme, "ssh" | "https" | "http" | "git") {
            return None;
        }
        rest.split_once('/')?
    } else {
        remote.split_once(':')?
    };
    let host = host
        .rsplit('@')
        .next()?
        .split(':')
        .next()?
        .to_ascii_lowercase();
    let path = path.trim_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{host}/{path}"))
}

/// A committed identity. Older files may omit the display name.
#[derive(Debug, Deserialize)]
pub struct ProjectFile {
    pub project_id: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// A malformed or unreadable existing file is an actionable warning, never a request to initialize.
pub fn read(dir: &Path) -> Result<Option<ProjectFile>, Error> {
    let Some(top) = toplevel(dir) else {
        return Ok(None);
    };
    let path = top.join(FILE);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(Error::Input(
                "cannot read .cairn/project.json; capture remains personal".to_owned(),
            ));
        }
    };
    let file: ProjectFile = serde_json::from_str(&raw).map_err(|_| {
        Error::Input("invalid .cairn/project.json; capture remains personal".to_owned())
    })?;
    if file.project_id.trim().is_empty()
        || file
            .name
            .as_deref()
            .is_some_and(|name| name.trim().is_empty())
    {
        return Err(Error::Input(
            "invalid .cairn/project.json; capture remains personal".to_owned(),
        ));
    }
    Ok(Some(file))
}

pub fn read_project_id(dir: &Path) -> Result<Option<String>, Error> {
    Ok(read(dir)?.map(|file| file.project_id))
}

/// The project id in `<git toplevel of dir>/.cairn/project.json`, if there is a readable one.
pub fn project_id(dir: &Path) -> Option<String> {
    match read_project_id(dir) {
        Ok(id) => id,
        Err(error) => {
            eprintln!("cairn: warning: {error}");
            None
        }
    }
}

/// Writes the server-confirmed identity and name. A different project or broken file is
/// refused; an unchanged file is left untouched. Returns true when the file was written.
pub fn write(top: &Path, project_id: &str, repository: &str, name: &str) -> Result<bool, Error> {
    if let Some(existing) = read(top)? {
        if existing.project_id != project_id {
            return Err(Error::Input(format!(
                "{FILE} already names project {}; not overwriting it with {project_id}",
                existing.project_id
            )));
        }
        if existing.name.as_deref() == Some(name) {
            return Ok(false);
        }
    }
    let path = top.join(FILE);
    let io =
        |error: std::io::Error| Error::Input(format!("cannot write {}: {error}", path.display()));
    fs::create_dir_all(path.parent().expect("the file sits in .cairn")).map_err(io)?;
    let body = serde_json::to_string_pretty(&json!({
        "project_id": project_id,
        "repository": repository,
        "name": name,
    }))
    .expect("JSON value serializes");
    crate::write_private(&path, format!("{body}\n").as_bytes()).map_err(io)?;
    Ok(true)
}

#[cfg(test)]
mod tests;
