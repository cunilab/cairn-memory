mod common;

use common::{Server, checkout, command, team, with_stdin};
use serde_json::{Value, json};
use std::fs;
use std::process::{Command, Output};

const SOURCE_SECRET: &str = "fake-secret-for-import-test";
const INDEX: &str = "# Original index\r\n\r\n- Review [project.md](project.md).\r\n";

fn success(output: Output) -> String {
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn import_preserves_sources_and_deduplicates_across_retries() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "claude-memory");
    let dir = checkout("claude-import");
    let agent = team.owner;
    success(agent.run_in(&dir, &["init", "E2E Claude import"]));
    let project_id = fs::read_to_string(dir.join(".cairn/project.json")).unwrap();
    let project_id: Value = serde_json::from_str(&project_id).unwrap();

    // Claude uses the main checkout's folder even when launched from a nested worktree folder.
    let worktree = dir.with_file_name("worktree");
    assert!(
        Command::new("git")
            .current_dir(&dir)
            .args(["worktree", "add", "-q", "--detach"])
            .arg(&worktree)
            .status()
            .unwrap()
            .success()
    );
    fs::create_dir_all(worktree.join(".cairn")).unwrap();
    fs::copy(
        dir.join(".cairn/project.json"),
        worktree.join(".cairn/project.json"),
    )
    .unwrap();
    let nested = worktree.join("nested");
    fs::create_dir(&nested).unwrap();

    let config = agent.home.join("custom-claude");
    let native = dir
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .replace(|c: char| !c.is_ascii_alphanumeric(), "-");
    let folder = config.join("projects").join(native).join("memory");
    fs::create_dir_all(&folder).unwrap();
    let index = folder.join("MEMORY.md");
    let backup = folder.join("MEMORY.md.pre-cairn");
    fs::write(&index, INDEX).unwrap();
    let note = |name: &str, title: &str, description: &str, kind: &str, body: &str| {
        fs::write(
            folder.join(name),
            format!("---\nname: {title}\ndescription: '{description}'\nmetadata:\n  node_type: memory\n  type: {kind}\n---\n\n{body}"),
        ).unwrap();
    };
    let why = "Releasing weekly gives the team a clear review window.";
    let body = format!(
        "# Release notes\n\n**Why:** {why}\n\n{}",
        "📌 release history ".repeat(80).trim_end()
    );
    let oversized = "📌 Fixture churn has hidden regressions during earlier changes. ".repeat(150);
    for (file, title, description, kind, reason) in [
        (
            "project.md",
            "Project release cadence",
            "Releases happen every Friday before the deployment window.",
            "project",
            body.as_str(),
        ),
        (
            "user.md",
            "Review presentation",
            "The user prefers short review notes with specific examples.",
            "user",
            "Examples make findings easier to act on.",
        ),
        (
            "feedback.md",
            "Preserve stable fixtures",
            "Keep fixture records stable so regressions are easy to compare.",
            "feedback",
            oversized.as_str(),
        ),
        (
            "reference.md",
            "Deployment guide",
            "The operational checklist is documented in the deployment guide.",
            "reference",
            "",
        ),
        (
            "thin.md",
            "Release checklist",
            "short",
            "reference",
            "The release notes cite change identifiers.",
        ),
        (
            "invalid.md",
            "Invalid empty summary",
            "",
            "reference",
            "The source is missing its description.",
        ),
        (
            "secret.md",
            "Secret example",
            "This note contains a credential that should never be sent.",
            "feedback",
            &format!("password={SOURCE_SECRET}"),
        ),
    ] {
        note(file, title, description, kind, reason);
    }
    let source = folder.join("project.md");
    let project_source = fs::read_to_string(&source).unwrap();
    let native_import = || {
        let mut cmd = command(&agent.home);
        cmd.current_dir(&nested)
            .env("CLAUDE_CONFIG_DIR", &config)
            .env_remove("CLAUDE_CODE_PROJECT_DIR_NAME")
            .args(["import", "claude-memory"]);
        cmd
    };
    let memories = || {
        team.owner_web
            .get("/cairn-memory/memories")
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    };
    let preview = success(with_stdin(&mut native_import(), "n"));
    assert!(preview.contains("File\tKind\tScope\tTitle") && preview.contains("project.md"));
    assert!(preview.contains("back up the old MEMORY.md"));
    assert!(preview.contains("secret.md") && !preview.contains(SOURCE_SECRET));
    assert!(memories().is_empty());
    assert_eq!(fs::read(&index).unwrap(), INDEX.as_bytes());
    assert!(!backup.exists());

    let partial = native_import().arg("--yes").output().unwrap();
    assert!(!partial.status.success());
    let error = String::from_utf8(partial.stderr).unwrap();
    assert!(
        error.contains("invalid.md") && error.contains("summary must not be empty"),
        "{error}"
    );
    assert_eq!(fs::read(&index).unwrap(), INDEX.as_bytes());
    assert!(!backup.exists());
    let stdout = String::from_utf8(partial.stdout).unwrap();
    assert!(stdout.contains("Saved \"thin.md\""));
    assert!(stdout.contains("Write a summary of at least five words"));
    assert!(stdout.contains("Improve it: save again with supersedes="));
    let read = memories();
    assert_eq!(read.len(), 5);
    let thin = read
        .iter()
        .find(|row| row["title"] == "Release checklist")
        .unwrap();
    assert_eq!(thin["trust_level"], "new");

    fs::remove_file(folder.join("invalid.md")).unwrap();
    let applied = success(with_stdin(&mut native_import(), "yes"));
    assert!(applied.contains("Skipped \"secret.md\" (possible secret)"));
    assert!(applied.contains("Clipped detail in \"feedback.md\""));
    assert!(!applied.contains("Clipped detail in \"project.md\""));
    assert!(!applied.contains(SOURCE_SECRET));
    assert_eq!(applied.matches("Already saved").count(), 5);
    let pointer = fs::read_to_string(&index).unwrap();
    assert!(pointer.contains("memory_search and memory_save"));
    assert_eq!(fs::read(&backup).unwrap(), INDEX.as_bytes());
    assert_eq!(fs::read_to_string(&source).unwrap(), project_source);

    let read = memories();
    assert_eq!(read.len(), 5);
    for (title, kind, scope) in [
        ("Project release cadence", "note", "project"),
        ("Review presentation", "preference", "personal"),
        ("Preserve stable fixtures", "rule", "personal"),
        ("Deployment guide", "fact", "personal"),
    ] {
        let row = read.iter().find(|row| row["title"] == title).unwrap();
        assert_eq!(row["kind"], kind);
        assert_eq!(row["scope"], scope);
        let id = row["id"].as_str().unwrap();
        let response = agent.json_in(&nested, &["get", id, "--detail"]);
        let full = &response["memories"][0];
        if scope == "project" {
            assert_eq!(row["project_id"], project_id["project_id"]);
            assert_eq!(full["evidence"], json!([source.to_str().unwrap()]));
            assert_eq!(full["reason"], why);
            assert_eq!(full["detail"], body);
            let ordinary = agent.json_in(&nested, &["get", id]);
            assert_eq!(
                ordinary["memories"][0]["detail_chars"],
                body.chars().count()
            );
            assert!(ordinary["memories"][0].get("detail").is_none());
        } else if kind == "rule" {
            let detail = full["detail"].as_str().unwrap();
            assert_eq!(detail.chars().count(), 8_000);
            assert!(detail.ends_with(&format!(
                "... (full text: {})",
                folder.join("feedback.md").display()
            )));
            assert_eq!(
                full["reason"],
                oversized.chars().take(1_000).collect::<String>().trim_end()
            );
        } else if title == "Deployment guide" {
            assert_eq!(full["reason"], "Imported from Claude memory");
        }
    }

    let again = success(native_import().arg("--yes").output().unwrap());
    assert_eq!(again.matches("Already saved").count(), 5);
    assert_eq!(memories().len(), 5);
    assert_eq!(fs::read_to_string(&index).unwrap(), pointer);
    assert_eq!(fs::read(&backup).unwrap(), INDEX.as_bytes());
    fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    fs::remove_dir_all(&agent.home).unwrap();
    fs::remove_dir_all(&team.member.home).unwrap();
}
