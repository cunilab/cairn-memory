//! The committed `.cairn/project.json`: which Cairn project a repository belongs to. It holds
//! only the project id and repository name, no secret and no server URL. A capture that carries
//! the id makes the capturer a member of the project.

use crate::Error;
use serde_json::{Value, json};
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

/// The repository name the hook derives for the same checkout: the last path part of
/// `origin` (no host, user or credentials), else the top-level directory name.
pub fn repository_name(top: &Path) -> String {
    let origin = git(top, &["config", "--get", "remote.origin.url"]).unwrap_or_default();
    let origin = origin.trim_end_matches('/');
    let origin = origin.strip_suffix(".git").unwrap_or(origin);
    let name = origin.replace(':', "/");
    match name.rsplit('/').next().filter(|name| !name.is_empty()) {
        Some(name) => name.to_owned(),
        None => top
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

/// The project id in `<git toplevel of dir>/.cairn/project.json`, if there is a readable one.
pub fn project_id(dir: &Path) -> Option<String> {
    existing_id(&toplevel(dir)?)
}

fn existing_id(top: &Path) -> Option<String> {
    let value: Value = serde_json::from_str(&fs::read_to_string(top.join(FILE)).ok()?).ok()?;
    value["project_id"]
        .as_str()
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
}

/// Writes the project file under `top`. A file that names a different project is kept and
/// refused; the same project is left as it is. Returns true when the file was written.
pub fn write(top: &Path, project_id: &str, repository: &str) -> Result<bool, Error> {
    if let Some(existing) = existing_id(top) {
        if existing == project_id {
            return Ok(false);
        }
        return Err(Error::Input(format!(
            "{FILE} already names project {existing}; not overwriting it with {project_id}"
        )));
    }
    let path = top.join(FILE);
    let io =
        |error: std::io::Error| Error::Input(format!("cannot write {}: {error}", path.display()));
    fs::create_dir_all(path.parent().expect("the file sits in .cairn")).map_err(io)?;
    let body = serde_json::to_string_pretty(&json!({
        "project_id": project_id,
        "repository": repository,
    }))
    .expect("JSON value serializes");
    fs::write(&path, format!("{body}\n")).map_err(io)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkout(name: &str, origin: Option<&str>) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cairn-project-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&dir)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        run(&["init", "-q"]);
        if let Some(origin) = origin {
            run(&["remote", "add", "origin", origin]);
        }
        dir.canonicalize().unwrap()
    }

    #[test]
    fn repository_name_follows_the_hook_convention() {
        for (origin, expected) in [
            ("https://example.test/acme/cairn-memory.git", "cairn-memory"),
            ("git@example.test:acme/cairn-memory.git/", "cairn-memory"),
            ("git@example.test:solo.git", "solo"),
        ] {
            let top = checkout("origin", Some(origin));
            assert_eq!(repository_name(&top), expected, "{origin}");
            let _ = fs::remove_dir_all(&top);
        }
        let top = checkout("no-origin", None);
        let expected = top.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(repository_name(&top), expected);
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn the_project_file_is_found_from_a_subdirectory_and_never_overwritten() {
        let top = checkout("file", None);
        let nested = top.join("src/deep");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(project_id(&nested), None);

        assert!(write(&top, "project-a", "repo").unwrap());
        assert_eq!(project_id(&nested).as_deref(), Some("project-a"));
        let raw = fs::read_to_string(top.join(FILE)).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&raw).unwrap(),
            json!({"project_id": "project-a", "repository": "repo"})
        );
        assert!(raw.contains("\n  \"project_id\""), "pretty JSON: {raw}");

        assert!(
            !write(&top, "project-a", "repo").unwrap(),
            "same id is a no-op"
        );
        let refused = write(&top, "project-b", "repo").unwrap_err();
        assert!(refused.to_string().contains("project-a"));
        assert_eq!(project_id(&top).as_deref(), Some("project-a"));
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn an_unreadable_or_empty_project_file_gives_no_id() {
        let top = checkout("broken", None);
        fs::create_dir_all(top.join(".cairn")).unwrap();
        for body in [
            "not json",
            "{}",
            r#"{"project_id": ""}"#,
            r#"{"project_id": 7}"#,
        ] {
            fs::write(top.join(FILE), body).unwrap();
            assert_eq!(project_id(&top), None, "{body}");
        }
        let _ = fs::remove_dir_all(&top);
    }
}
