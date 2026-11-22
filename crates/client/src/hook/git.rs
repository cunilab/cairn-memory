//! Repository facts for the hook, read with `git`: work context, recent files, repo-relative
//! paths and the `.cairnignore` marker. Failure of any call just leaves the fact out.

use cairn_core::capture::WorkContext;
use cairn_core::id::sha256_hex;
use serde_json::Value;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const RECENT_FILES_MAX: usize = 20;

const GIT_TIMEOUT: Duration = Duration::from_secs(2);
/// Set when one call timed out: the rest of this hook skips git instead of waiting again.
static STUCK: AtomicBool = AtomicBool::new(false);

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    git_raw(cwd, args).map(|text| text.trim().to_owned())
}

/// Read-only: `GIT_OPTIONAL_LOCKS=0` keeps `git status` from refreshing the index, which takes
/// `.git/index.lock` and would fail another agent's `git add` in the same checkout. A call that
/// does not finish in two seconds is killed.
fn git_raw(cwd: &Path, args: &[&str]) -> Option<String> {
    let (code, text) = git_run(cwd, args, GIT_TIMEOUT)?;
    (code == Some(0)).then_some(text)
}

/// The exit code (`None` when a signal ended it) and stdout of a call; `None` when it could not
/// start or did not finish within `timeout` (which also stops the rest of this hook's git calls).
fn git_run(cwd: &Path, args: &[&str], timeout: Duration) -> Option<(Option<i32>, String)> {
    if STUCK.load(Ordering::Relaxed) {
        return None;
    }
    let mut child = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait().ok()? {
            Some(status) => break status,
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                STUCK.store(true, Ordering::Relaxed);
                return None;
            }
        }
    };
    let bytes = reader.join().ok()?.ok()?;
    Some((status.code(), String::from_utf8_lossy(&bytes).into_owned()))
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
pub fn work_context(cwd: &Path, top: Option<&Path>) -> WorkContext {
    let Some(top) = top else {
        return WorkContext::default();
    };
    let origin = git(cwd, &["config", "--get", "remote.origin.url"]).unwrap_or_default();
    let branch = git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .filter(|branch| !branch.is_empty() && branch != "HEAD");
    let hash = sha256_hex(&[&top.to_string_lossy()]);
    WorkContext {
        project_id: project_id(top),
        repository: Some(repository_name(&origin, top)),
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

const SYMBOL_GREP_TIMEOUT: Duration = Duration::from_millis(1500);
const MERGED_BRANCHES_MAX: usize = 50;

/// What the checkout answers to a brief's `check` (CM-SPEC-008 §7.2).
pub struct RepoFacts {
    pub commit: Option<String>,
    pub files_missing: Vec<String>,
    pub files_present: Vec<String>,
    pub symbols_missing: Vec<String>,
    pub symbols_present: Vec<String>,
    pub merged_branches: Vec<String>,
}

/// Files are looked up among tracked and untracked-but-not-ignored paths (one process), symbols
/// with one `git grep` (a timeout leaves them unreported), and merged branches from
/// `git branch --merged HEAD` minus the current one. `None` when git cannot list the files.
pub fn repo_facts(top: &Path, files: &[String], symbols: &[String]) -> Option<RepoFacts> {
    let listed = git_raw(
        top,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?;
    let known: std::collections::HashSet<&str> = listed.split('\0').collect();
    let (mut files_present, mut files_missing) = (Vec::new(), Vec::new());
    for file in files {
        // A name outside the checkout is never reported, present or missing.
        if outside(file) {
            continue;
        }
        if known.contains(file.as_str()) || top.join(file).exists() {
            files_present.push(file.clone());
        } else if !undecidable(top, file, &known) {
            files_missing.push(file.clone());
        }
    }
    let commit = git(top, &["rev-parse", "HEAD"]).filter(|c| !c.is_empty());
    let current = git(top, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let merged_branches = git_raw(
        top,
        &["branch", "--merged", "HEAD", "--format=%(refname:short)"],
    )
    .map(|names| {
        names
            .lines()
            .map(str::trim)
            .filter(|name| !name.is_empty() && !name.starts_with('(') && *name != current)
            .take(MERGED_BRANCHES_MAX)
            .map(str::to_owned)
            .collect()
    })
    .unwrap_or_default();
    let (symbols_present, symbols_missing) = match symbol_matches(top, symbols) {
        Some(found) => symbols
            .iter()
            .cloned()
            .partition(|symbol| found.contains(symbol.as_str())),
        None => Default::default(),
    };
    Some(RepoFacts {
        commit,
        files_missing,
        files_present,
        symbols_missing,
        symbols_present,
        merged_branches,
    })
}

fn outside(file: &str) -> bool {
    file.starts_with(['/', '~']) || file.contains(':') || file.split('/').any(|part| part == "..")
}

/// A name that is not in the checkout says nothing about the memory that cites it, when it
/// is gitignored (so another machine may not have it), or is written
/// relative to a subdirectory (it is the tail of a tracked path). Such a name is left unreported.
fn undecidable(top: &Path, file: &str, known: &std::collections::HashSet<&str>) -> bool {
    git_run(
        top,
        &["check-ignore", "-q", "--no-index", "--", file],
        GIT_TIMEOUT,
    )
    .is_none_or(|(code, _)| code != Some(1))
        || known.iter().any(|path| path.ends_with(&format!("/{file}")))
}

/// The symbols that occur as whole words in tracked files; `None` on a timeout or a git error.
fn symbol_matches(top: &Path, symbols: &[String]) -> Option<std::collections::HashSet<String>> {
    if symbols.is_empty() {
        return Some(Default::default());
    }
    let mut args = vec!["grep", "-h", "-o", "-w", "-F"];
    for symbol in symbols {
        args.extend(["-e", symbol]);
    }
    // Exit 1 means "no match", which is an answer.
    let (code, text) = git_run(top, &args, SYMBOL_GREP_TIMEOUT)?;
    matches!(code, Some(0 | 1)).then(|| text.lines().map(str::to_owned).collect())
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
pub fn repo_relative(value: &str, cwd: &Path, top: Option<&Path>) -> Option<String> {
    if value.is_empty() || value.contains('\n') || value.len() > 512 {
        return None;
    }
    let top = top?;
    let given = Path::new(value);
    let absolute = if given.is_absolute() {
        given.to_path_buf()
    } else {
        cwd.join(given)
    };
    let relative = resolve(&absolute).strip_prefix(top).ok()?.to_path_buf();
    let text = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    (!text.is_empty()).then_some(text)
}

/// `.cairnignore` in the folder or any ancestor up to the git top (every ancestor outside git).
pub fn ignored(cwd: &Path, top: Option<&Path>) -> bool {
    let folder = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    for parent in folder.ancestors() {
        if parent.join(".cairnignore").is_file() {
            return true;
        }
        if top == Some(parent) {
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
    fn a_name_git_cannot_vouch_for_is_never_reported_missing() {
        let dir = std::env::temp_dir().join(format!("cairn-git-facts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("crates/x/src")).unwrap();
        std::fs::write(dir.join("crates/x/src/a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "local.toml\n").unwrap();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&dir)
                    .args(["-c", "user.name=t", "-c", "user.email=t@example.test"])
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        run(&["init", "-q"]);
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "x"]);
        let asked: Vec<String> = [
            "crates/x/src/a.rs",
            "gone/file.rs",
            "config/local.toml",
            "src/a.rs",
            "/etc/nginx/nginx.conf",
            "../other/README.md",
        ]
        .map(str::to_owned)
        .into_iter()
        // An absolute path that exists on this machine is still outside the checkout's names.
        .chain([dir.join("crates/x/src/a.rs").to_string_lossy().into_owned()])
        .collect();
        let facts = repo_facts(&dir, &asked, &[]).unwrap();
        assert_eq!(facts.files_present, ["crates/x/src/a.rs"]);
        assert_eq!(facts.files_missing, ["gone/file.rs"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reading_the_status_never_rewrites_the_index() {
        // A plain `git status` refreshes stale stat data and takes `.git/index.lock` to save it,
        // which would fail another agent's `git add` in the same checkout.
        let dir = std::env::temp_dir().join(format!("cairn-git-locks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&dir)
                    .args(["-c", "user.name=t", "-c", "user.email=t@example.test"])
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        run(&["init", "-q"]);
        std::fs::write(dir.join("a.txt"), "same").unwrap();
        run(&["add", "a.txt"]);
        run(&["commit", "-q", "-m", "one"]);
        // Same content, newer mtime: a refresh would rewrite the index.
        std::fs::File::options()
            .write(true)
            .open(dir.join("a.txt"))
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
            .unwrap();
        let before = std::fs::read(dir.join(".git/index")).unwrap();
        recent_files(&dir);
        assert_eq!(std::fs::read(dir.join(".git/index")).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
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
        let top = top_level(&dir);
        let relative = |value: &str| repo_relative(value, &dir, top.as_deref());
        assert_eq!(relative("src/new.rs").as_deref(), Some("src/new.rs"));
        assert_eq!(relative("src/../a/./b.rs").as_deref(), Some("a/b.rs"));
        let absolute = dir.join("src/x.rs");
        assert_eq!(
            relative(&absolute.to_string_lossy()).as_deref(),
            Some("src/x.rs")
        );
        assert_eq!(relative("../outside.rs"), None);
        assert_eq!(relative("/etc/passwd"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
