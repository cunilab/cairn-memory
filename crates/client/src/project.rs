//! The committed `.cairn/project.json`: which Cairn project a repository belongs to. It holds
//! the project id, repository and display name, no secret and no server URL. A capture that carries
//! the id makes the capturer a member of the project.

use crate::Error;
use cairn_core::id::sha256_hex;
use serde::Deserialize;
use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const FILE: &str = ".cairn/project.json";

pub(crate) fn git(dir: &Path, args: &[&str]) -> Option<String> {
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

/// Opaque identity of the canonical checkout root, shared by capture and recall.
pub(crate) fn worktree_id(top: &Path) -> String {
    let hash = sha256_hex(&[&top.to_string_lossy()]);
    format!("wt-{}", &hash[..16])
}

/// Without git: the nearest ancestor of `cwd` (inclusive) that contains `.cairn/project.json`,
/// stopping at `$HOME` or `/`.
pub fn find_root(cwd: &Path) -> Option<PathBuf> {
    let start = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .and_then(|path| path.canonicalize().ok().or(Some(path)));
    for dir in start.ancestors() {
        if dir.join(FILE).is_file() {
            return Some(dir.to_path_buf());
        }
        if dir == Path::new("/") || home.as_ref().is_some_and(|home| dir == home.as_path()) {
            break;
        }
    }
    None
}

/// True when `.cairn/project.json` is already tracked. A failure (untracked, or not a git checkout)
/// is false; this never stages or commits.
pub fn project_file_tracked(top: &Path) -> bool {
    git(top, &["ls-files", "--error-unmatch", FILE]).is_some()
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
    match toplevel(dir).or_else(|| find_root(dir)) {
        Some(top) => read_at(&top),
        None => Ok(None),
    }
}

/// `.cairn/project.json` in `top` itself.
pub fn read_at(top: &Path) -> Result<Option<ProjectFile>, Error> {
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

/// The project id in `<git toplevel of dir>/.cairn/project.json`, if there is a readable one.
pub fn project_id(dir: &Path) -> Option<String> {
    project_id_at(&toplevel(dir).or_else(|| find_root(dir))?)
}

/// Same, for a git toplevel the caller already has (saves a `git` process).
pub fn project_id_at(top: &Path) -> Option<String> {
    match read_at(top).map(|file| file.map(|file| file.project_id)) {
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
    let path = top.join(FILE);
    let io =
        |error: std::io::Error| Error::Input(format!("cannot write {}: {error}", path.display()));
    let body = if let Some(existing) = read_at(top)? {
        if existing.project_id != project_id {
            return Err(Error::Input(format!(
                "{FILE} already names project {}; not overwriting it with {project_id}",
                existing.project_id
            )));
        }
        if existing.name.as_deref() == Some(name) {
            return Ok(false);
        }
        let raw = fs::read_to_string(&path).map_err(io)?;
        let fields: std::collections::HashMap<String, &serde_json::value::RawValue> =
            serde_json::from_str(&raw)
                .map_err(|_| Error::Input("invalid .cairn/project.json".into()))?;
        let encoded = serde_json::to_string(name).expect("name serializes");
        if let Some(value) = fields.get("name") {
            let start = value.get().as_ptr() as usize - raw.as_ptr() as usize;
            format!(
                "{}{}{}",
                &raw[..start],
                encoded,
                &raw[start + value.get().len()..]
            )
        } else {
            let end = raw
                .rfind('}')
                .ok_or_else(|| Error::Input("invalid .cairn/project.json".into()))?;
            format!("{},\"name\":{}{}", &raw[..end], encoded, &raw[end..])
        }
    } else {
        fs::create_dir_all(path.parent().expect("the file sits in .cairn")).map_err(io)?;
        project_body(project_id, repository, name)
    };
    crate::write_private(&path, body.as_bytes()).map_err(io)?;
    Ok(true)
}

#[cfg(test)]
mod tests;

fn project_body(project_id: &str, repository: &str, name: &str) -> String {
    let body = serde_json::to_string_pretty(
        &json!({"project_id":project_id,"repository":repository,"name":name}),
    )
    .expect("JSON value serializes");
    format!("{body}\n")
}

/// Create the project file once, without replacing an existing file or symlink.
pub fn write_new(
    top: &Path,
    project_id: &str,
    repository: &str,
    name: &str,
) -> std::io::Result<()> {
    let top = top.canonicalize()?;
    let dir = top.join(".cairn");
    fs::create_dir_all(&dir)?;
    let dir = dir.canonicalize()?;
    if !dir.starts_with(&top) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "outside checkout",
        ));
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(dir.join("project.json"))?;
    file.write_all(project_body(project_id, repository, name).as_bytes())?;
    file.sync_all()
}
