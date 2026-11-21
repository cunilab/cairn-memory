//! Repository facts for the hook, read with `git`: work context, recent files, repo-relative
//! paths and the `.cairnignore` marker. Failure of any call just leaves the fact out.

use cairn_core::capture::WorkContext;
use cairn_core::id::sha256_hex;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

const RECENT_FILES_MAX: usize = 20;

pub fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    git_raw(cwd, args).map(|text| text.trim().to_owned())
}

fn git_raw(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn top_level(cwd: &Path) -> Option<PathBuf> {
    git(cwd, &["rev-parse", "--show-toplevel"])
        .filter(|top| !top.is_empty())
        .and_then(|top| Path::new(&top).canonicalize().ok())
}

/// `host/owner/repo` from `origin` (no credentials), else `local:<top directory name>`.
fn repository_name(origin: &str, top: &Path) -> String {
    let remote = origin.trim_end_matches('/');
    let remote = remote.strip_suffix(".git").unwrap_or(remote);
    let (host, path) = match remote.split_once("://") {
        Some((scheme, rest)) if ["ssh", "https", "http", "git"].contains(&scheme) => {
            rest.split_once('/').unwrap_or((rest, ""))
        }
        Some(_) => ("", ""),
        None => remote.split_once(':').unwrap_or((remote, "")),
    };
    let host = host.rsplit('@').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("").to_lowercase();
    let path = path.trim_matches('/');
    if host.is_empty() || path.is_empty() {
        let name = top
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        format!("local:{name}")
    } else {
        format!("{host}/{path}")
    }
}

/// Repository, branch, an opaque id of this checkout and the committed project id; empty
/// outside a repository. A broken `.cairn/project.json` keeps the capture personal and says
/// so on stderr.
pub fn work_context(cwd: &Path) -> WorkContext {
    let Some(top) = top_level(cwd) else {
        return WorkContext::default();
    };
    let origin = git(cwd, &["config", "--get", "remote.origin.url"]).unwrap_or_default();
    let branch = git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .filter(|branch| !branch.is_empty() && branch != "HEAD");
    let hash = sha256_hex(&[&top.to_string_lossy()]);
    WorkContext {
        project_id: project_id(&top),
        repository: Some(repository_name(&origin, &top)),
        branch,
        worktree: Some(format!("wt-{}", &hash[..16])),
        ..WorkContext::default()
    }
}

fn project_id(top: &Path) -> Option<String> {
    let warn = |reason: &str| eprintln!("Cairn warning: {reason}; capture remains personal");
    let text = match std::fs::read_to_string(top.join(".cairn").join("project.json")) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => {
            warn("cannot read valid .cairn/project.json");
            return None;
        }
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        warn("cannot read valid .cairn/project.json");
        return None;
    };
    let id = value
        .get("project_id")
        .and_then(Value::as_str)
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty());
    if id.is_none() {
        warn("invalid .cairn/project.json project id");
    }
    id
}

/// `git status` paths (untracked included) then the last five commits' files: repo-relative,
/// deduplicated, at most 20.
pub fn recent_files(cwd: &Path) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    let mut add = |path: &str| {
        if !path.is_empty() && files.len() < RECENT_FILES_MAX && !files.iter().any(|f| f == path) {
            files.push(path.to_owned());
        }
    };
    if let Some(status) = git_raw(cwd, &["status", "--porcelain=v1", "-z"]) {
        let mut entries = status.split('\0');
        while let Some(entry) = entries.next() {
            add(entry.get(3..).unwrap_or(""));
            // A rename or copy is followed by its source path.
            if entry
                .bytes()
                .take(2)
                .any(|status| matches!(status, b'R' | b'C'))
            {
                entries.next();
            }
        }
    }
    if let Some(log) = git_raw(
        cwd,
        &[
            "log",
            "-n",
            "5",
            "--name-only",
            "--format=",
            "--no-show-signature",
        ],
    ) {
        log.lines().for_each(|line| add(line.trim()));
    }
    files
}

/// Canonicalizes the longest existing prefix and appends the rest, so a file the tool is
/// about to create (or just deleted) still resolves through a symlinked directory.
fn resolve(path: &Path) -> PathBuf {
    let mut clean = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            other => clean.push(other),
        }
    }
    let mut tail = Vec::new();
    let mut base = clean.clone();
    while !base.exists() {
        match base.file_name() {
            Some(name) => tail.push(name.to_owned()),
            None => break,
        }
        base.pop();
    }
    let mut resolved = base.canonicalize().unwrap_or(base);
    resolved.extend(tail.into_iter().rev());
    resolved
}

/// A structured tool path as a `/`-separated path inside the checkout, else `None`.
pub fn repo_relative(value: &str, cwd: &Path) -> Option<String> {
    if value.is_empty() || value.contains('\n') || value.len() > 512 {
        return None;
    }
    let top = top_level(cwd)?;
    let given = Path::new(value);
    let absolute = if given.is_absolute() {
        given.to_path_buf()
    } else {
        cwd.join(given)
    };
    let relative = resolve(&absolute).strip_prefix(&top).ok()?.to_path_buf();
    let text = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    (!text.is_empty()).then_some(text)
}

/// `.cairnignore` in the folder or any ancestor up to the git top (every ancestor outside git).
pub fn ignored(cwd: &Path) -> bool {
    let folder = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let top = top_level(&folder);
    for parent in folder.ancestors() {
        if parent.join(".cairnignore").is_file() {
            return true;
        }
        if top.as_deref() == Some(parent) {
            break;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_names_come_from_origin_without_credentials() {
        let top = Path::new("/work/cairn");
        for (origin, name) in [
            ("git@github.com:acme/cairn.git", "github.com/acme/cairn"),
            (
                "https://user:pw@GitHub.com/acme/cairn/",
                "github.com/acme/cairn",
            ),
            (
                "ssh://git@host.example:2222/acme/cairn.git",
                "host.example/acme/cairn",
            ),
            ("", "local:cairn"),
            ("file:///srv/cairn.git", "local:cairn"),
            ("https://github.com", "local:cairn"),
        ] {
            assert_eq!(repository_name(origin, top), name, "{origin}");
        }
    }

    #[test]
    fn a_missing_file_resolves_inside_the_checkout_only() {
        let dir = std::env::temp_dir().join(format!("cairn-git-resolve-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            repo_relative("src/new.rs", &dir).as_deref(),
            Some("src/new.rs")
        );
        assert_eq!(
            repo_relative("src/../a/./b.rs", &dir).as_deref(),
            Some("a/b.rs")
        );
        let absolute = dir.join("src/x.rs");
        assert_eq!(
            repo_relative(&absolute.to_string_lossy(), &dir).as_deref(),
            Some("src/x.rs")
        );
        assert_eq!(repo_relative("../outside.rs", &dir), None);
        assert_eq!(repo_relative("/etc/passwd", &dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
