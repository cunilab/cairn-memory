//! Claude's native memory directory, one-time import and local write guard.

use crate::{Client, Config, Error, project, tools, write_private};
use cairn_core::capture::redact_sensitive_content as redact;
use cairn_core::id::new_id;
use cairn_core::memory::validate::{DETAIL_MAX, REASON_MAX, SUMMARY_MAX, TITLE_MAX};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::{env, fs};

pub(crate) const WRITE_TOOLS: &str = "Write|Edit|MultiEdit|NotebookEdit";
const INDEX: &str = "MEMORY.md";
const BACKUP: &str = "MEMORY.md.pre-cairn";
const POINTER: &[u8] = b"Memory for this project lives in Cairn. Use memory_search and memory_save. The original files remain on disk.\n";

pub(crate) fn config_dir(home: &Path) -> PathBuf {
    env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| home.join(".claude"), PathBuf::from)
}

fn home() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|dir| !dir.is_empty())
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn normalized(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            _ => result.push(part),
        }
    }
    result
}

fn resolve(path: &Path) -> PathBuf {
    // A write can target a file that does not exist. Resolve any symlink in its existing parent.
    path.ancestors()
        .find_map(|parent| {
            Some(normalized(
                &parent
                    .canonicalize()
                    .ok()?
                    .join(path.strip_prefix(parent).ok()?),
            ))
        })
        .unwrap_or_else(|| normalized(path))
}

pub(crate) fn is_memory_path(path: &Path, cwd: Option<&Path>) -> bool {
    let Some(home) = home() else { return false };
    let path = if let Ok(relative) = path.strip_prefix("~") {
        home.join(relative)
    } else if let Some(cwd) = cwd {
        cwd.join(path)
    } else {
        path.to_path_buf()
    };
    let Ok(path) = std::path::absolute(path) else {
        return false;
    };
    let Ok(projects) = std::path::absolute(config_dir(&home).join("projects")) else {
        return false;
    };
    let inside = |base: &Path, file: &Path| {
        let Ok(relative) = file.strip_prefix(base) else {
            return false;
        };
        let mut parts = relative.components();
        matches!(parts.next(), Some(Component::Normal(_)))
            && parts.next() == Some(Component::Normal("memory".as_ref()))
            && parts.next().is_some()
    };
    inside(&normalized(&projects), &normalized(&path))
        || inside(&resolve(&projects), &resolve(&path))
}

/// Claude replaces each non-ASCII-alphanumeric UTF-16 unit with a dash. For long
/// paths it keeps 200 characters and appends the absolute signed hash in base 36.
fn project_folder(path: &str) -> String {
    let mut name = String::new();
    let mut hash = 0i32;
    for unit in path.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(i32::from(unit));
        let ascii = char::from_u32(u32::from(unit)).filter(char::is_ascii_alphanumeric);
        name.push(ascii.unwrap_or('-'));
    }
    if name.len() > 200 {
        name.truncate(200);
        name.push('-');
        let mut value = hash.unsigned_abs();
        let mut digits = Vec::new();
        loop {
            digits.push(char::from_digit(value % 36, 36).expect("a base-36 digit"));
            value /= 36;
            if value == 0 {
                break;
            }
        }
        name.extend(digits.into_iter().rev());
    }
    name
}

fn directory(cwd: &Path, home: &Path) -> Result<PathBuf, Error> {
    let config = std::path::absolute(config_dir(home)).map_err(|error| {
        Error::Input(format!("cannot resolve Claude config directory: {error}"))
    })?;
    if env::var_os("CLAUDE_CONFIG_DIR").is_some_and(|dir| !dir.is_empty())
        && let Ok(name) = env::var("CLAUDE_CODE_PROJECT_DIR_NAME")
        && valid_project_dir_name(&name)
    {
        return Ok(config.join("projects").join(name).join("memory"));
    }
    // Claude shares auto memory across worktrees. The first Git worktree is the main checkout.
    let root = project::git(cwd, &["worktree", "list", "--porcelain", "-z"])
        .as_deref()
        .and_then(|text| text.split('\0').next()?.strip_prefix("worktree "))
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.to_owned());
    let root = root.canonicalize().unwrap_or(root);
    let folder = root
        .to_str()
        .ok_or_else(|| Error::Input("Claude project path must be UTF-8".into()))?;
    Ok(config
        .join("projects")
        .join(project_folder(folder))
        .join("memory"))
}

fn valid_project_dir_name(name: &str) -> bool {
    if !(1..=64).contains(&name.len())
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
    {
        return false;
    }
    let lower = name.to_ascii_lowercase();
    let device = lower.len() == 4
        && (lower.starts_with("com") || lower.starts_with("lpt"))
        && lower.as_bytes()[3].is_ascii_digit();
    !device && !matches!(lower.as_str(), "con" | "prn" | "aux" | "nul")
}

#[derive(Deserialize)]
struct Frontmatter {
    #[serde(default)]
    name: String,
    description: String,
    metadata: Metadata,
}

#[derive(Deserialize)]
struct Metadata {
    #[serde(rename = "type")]
    kind: String,
}

struct ImportRecord {
    args: Map<String, Value>,
    clipped: bool,
}

fn frontmatter(raw: &str) -> Result<(Frontmatter, &str), &'static str> {
    let (first, remaining) = raw.split_once('\n').ok_or("missing YAML frontmatter")?;
    if first.trim_end_matches('\r') != "---" {
        return Err("missing YAML frontmatter");
    }
    let mut offset = 0;
    for line in remaining.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            let front = yaml_serde::from_str(&remaining[..offset])
                .map_err(|_| "invalid YAML frontmatter")?;
            let body = &remaining[offset + line.len()..];
            return Ok((
                front,
                body.strip_prefix("\r\n")
                    .or_else(|| body.strip_prefix('\n'))
                    .unwrap_or(body),
            ));
        }
        offset += line.len();
    }
    Err("missing closing YAML frontmatter delimiter")
}

fn import_record(
    raw: &str,
    source: &str,
    has_project: bool,
) -> Result<Option<ImportRecord>, &'static str> {
    if redact(raw) != raw || redact(source) != source {
        return Ok(None);
    }
    let (front, body) = frontmatter(raw.trim_start_matches('\u{feff}'))?;
    if [&front.name, &front.description]
        .into_iter()
        .any(|text| redact(text) != *text)
    {
        return Ok(None);
    }
    let (kind, scope) = match front.metadata.kind.as_str() {
        "user" => ("preference", "personal"),
        "feedback" => ("rule", "personal"),
        "project" if has_project => ("note", "project"),
        "project" => ("note", "personal"),
        "reference" => ("fact", "personal"),
        _ => return Err("unknown Claude memory type"),
    };
    let description = front.description.trim();
    let title = match front.name.trim() {
        "" => description,
        name => name,
    };
    let clipped = body.chars().count() > DETAIL_MAX;
    let detail = if clipped {
        let marker = format!("... (full text: {source})");
        if marker.chars().count() >= DETAIL_MAX {
            return Err("source path is too long to include in detail");
        }
        let prefix = body
            .chars()
            .take(DETAIL_MAX - marker.chars().count())
            .collect::<String>();
        format!("{prefix}{marker}")
    } else {
        body.into()
    };
    let reason = body
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("**Why:**").map(str::trim))
        .find(|why| !why.is_empty())
        .unwrap_or(if body.trim().is_empty() {
            "Imported from Claude memory"
        } else {
            body.trim()
        });
    let mut record = Map::new();
    record.insert("kind".into(), json!(kind));
    record.insert("scope".into(), json!(scope));
    record.insert(
        "title".into(),
        json!(title.chars().take(TITLE_MAX).collect::<String>()),
    );
    record.insert(
        "summary".into(),
        json!(description.chars().take(SUMMARY_MAX).collect::<String>()),
    );
    record.insert(
        "reason".into(),
        json!(reason.chars().take(REASON_MAX).collect::<String>()),
    );
    record.insert("detail".into(), json!(detail));
    record.insert("evidence".into(), json!([source]));
    Ok(Some(ImportRecord {
        args: record,
        clipped,
    }))
}

fn read_index(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => fs::read(path).map(Some),
        Ok(_) => Err(io::Error::other("expected a regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn index_without_pointer(cwd: &Path, home: &Path) -> Option<PathBuf> {
    let folder = directory(cwd, home).ok()?;
    if !folder.is_dir() {
        return None;
    }
    let index = folder.join(INDEX);
    (!read_index(&index).is_ok_and(|content| content.as_deref() == Some(POINTER))).then_some(index)
}

fn replace_index(folder: &Path, old: Option<&[u8]>) -> io::Result<()> {
    let index = folder.join(INDEX);
    if read_index(&index)?.as_deref() != old {
        return Err(io::Error::other("index changed during import; retry"));
    }
    if old == Some(POINTER) {
        return Ok(());
    }
    if let Some(old) = old {
        let backup = folder.join(BACKUP);
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        match options.open(&backup) {
            Ok(mut file) => {
                if let Err(error) = file.write_all(old).and_then(|()| file.sync_all()) {
                    drop(file);
                    let _ = fs::remove_file(backup);
                    return Err(error);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if read_index(&backup)?.as_deref() != Some(old) {
                    return Err(io::Error::other(
                        "existing MEMORY.md.pre-cairn differs from the index",
                    ));
                }
            }
            Err(error) => return Err(error),
        }
    }
    let staged = folder.join(new_id("cairn_index"));
    write_private(&staged, POINTER)?;
    let replace = || {
        if read_index(&index)?.as_deref() != old {
            return Err(io::Error::other("index changed during import; retry"));
        }
        fs::rename(&staged, index)
    };
    replace().inspect_err(|_| {
        let _ = fs::remove_file(staged);
    })
}

/// Preview and import the current checkout's Claude notes using the same API as CLI saves.
pub fn import(yes: bool) -> Result<(), Error> {
    let cwd = env::current_dir().map_err(|error| Error::Input(error.to_string()))?;
    let home = home().ok_or_else(|| Error::Input("no home directory (set HOME)".into()))?;
    let folder = directory(&cwd, &home)?;
    let local_error = |error: io::Error| Error::Input(format!("Claude memory import: {error}"));
    let mut entries = fs::read_dir(&folder)
        .map_err(|error| {
            Error::Input(format!(
                "cannot open Claude memory folder {}: {error}",
                redact(&folder.display().to_string())
            ))
        })?
        .collect::<io::Result<Vec<_>>>()
        .map_err(local_error)?;
    entries.sort_by_key(fs::DirEntry::file_name);
    let index = read_index(&folder.join(INDEX)).map_err(local_error)?;
    let has_project = project::read(&cwd)?.is_some();
    let mut pending = Vec::new();
    let mut failed = false;
    if !yes {
        println!("File\tKind\tScope\tTitle");
    }
    for entry in entries {
        let path = entry.path();
        if path.file_name().is_some_and(|name| name == INDEX)
            || path.extension().is_none_or(|ext| ext != "md")
        {
            continue;
        }
        let source = path.to_string_lossy().into_owned();
        let file_name = entry.file_name();
        let label = redact(&file_name.to_string_lossy());
        let record = match entry.file_type() {
            Ok(kind) if kind.is_dir() => continue,
            Ok(kind) if kind.is_file() => match fs::read_to_string(&path) {
                Ok(raw) => import_record(&raw, &source, has_project),
                Err(_) => Err("cannot read UTF-8 source"),
            },
            _ => Err("source must be a regular file"),
        };
        match record {
            Ok(Some(ImportRecord {
                args: record,
                clipped,
            })) => {
                if !yes {
                    println!(
                        "{label:?}\t{}\t{}\t{}",
                        record["kind"], record["scope"], record["title"]
                    );
                }
                if clipped {
                    println!(
                        "Clipped detail in {label:?}. The full body stays in the source file."
                    );
                }
                pending.push((label, record));
            }
            Ok(None) => println!("Skipped {label:?} (possible secret)"),
            Err(error) => {
                eprintln!("{label:?}: {error}");
                failed = true;
            }
        }
    }
    if !yes {
        let prompt = if failed {
            "Save the listed files? MEMORY.md will stay in place while errors remain. [y/N] "
        } else {
            "Save the listed files, back up the old MEMORY.md when present, and replace it with a Cairn pointer? [y/N] "
        };
        print!("{prompt}");
        io::stdout().flush().map_err(local_error)?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer).map_err(local_error)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Cancelled.");
            return Ok(());
        }
    }
    if !pending.is_empty() {
        let client = Client::new(Config::load()?);
        let save = tools::find("save").expect("registered save tool");
        for (label, record) in pending {
            match tools::call(&client, save, record) {
                Ok(reply) if reply["id"].as_str().is_some_and(|id| !id.is_empty()) => {
                    let verb = if reply["deduplicated"] == true {
                        "Already saved"
                    } else {
                        "Saved"
                    };
                    println!("{verb} {label:?}");
                    for hint in reply["hints"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                    {
                        println!("  {}", redact(hint));
                    }
                    if let Some(improve) = reply["improve"].as_str() {
                        println!("  {}", redact(improve));
                    }
                }
                Ok(_) => {
                    eprintln!("{label:?}: invalid save receipt");
                    failed = true;
                }
                Err(error) => {
                    eprintln!("{label:?}: {}", redact(&error.to_string()));
                    failed = true;
                }
            }
        }
    }
    if failed {
        return Err(Error::Input(
            "Import incomplete. MEMORY.md was left in place. Fix the listed errors and re-run."
                .into(),
        ));
    }
    replace_index(&folder, index.as_deref()).map_err(local_error)?;
    println!("Claude memory index points to Cairn.");
    Ok(())
}

#[cfg(test)]
mod tests;
