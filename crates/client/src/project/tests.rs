use super::*;
use serde_json::Value;

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
        (
            "https://example.test/acme/cairn-memory.git",
            "example.test/acme/cairn-memory",
        ),
        (
            "git@example.test:acme/cairn-memory.git/",
            "example.test/acme/cairn-memory",
        ),
        ("git@example.test:solo.git", "example.test/solo"),
        (
            "ssh://git@EXAMPLE.test:22/acme/repo.git",
            "example.test/acme/repo",
        ),
        (
            "https://fixture:credential@EXAMPLE.test/acme/repo.git",
            "example.test/acme/repo",
        ),
    ] {
        let top = checkout("origin", Some(origin));
        assert_eq!(repository_name(&top), expected, "{origin}");
        let _ = fs::remove_dir_all(&top);
    }
    let top = checkout("no-origin", None);
    let expected = format!("local:{}", top.file_name().unwrap().to_string_lossy());
    assert_eq!(repository_name(&top), expected);
    let _ = fs::remove_dir_all(&top);
}

#[test]
fn the_project_file_is_found_from_a_subdirectory_and_never_overwritten() {
    let top = checkout("file", None);
    let nested = top.join("src/deep");
    fs::create_dir_all(&nested).unwrap();
    assert_eq!(project_id(&nested), None);

    assert!(write(&top, "project-a", "repo", "Project A").unwrap());
    assert_eq!(project_id(&nested).as_deref(), Some("project-a"));
    let raw = fs::read_to_string(top.join(FILE)).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&raw).unwrap(),
        json!({"project_id": "project-a", "repository": "repo", "name": "Project A"})
    );
    assert!(raw.contains("\n  \"project_id\""), "pretty JSON: {raw}");

    assert!(
        !write(&top, "project-a", "repo", "Project A").unwrap(),
        "same id is a no-op"
    );
    let refused = write(&top, "project-b", "repo", "Project B").unwrap_err();
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

#[test]
fn legacy_files_are_read_and_backfilled_and_broken_files_are_kept() {
    let top = checkout("legacy", None);
    fs::create_dir_all(top.join(".cairn")).unwrap();
    fs::write(
        top.join(FILE),
        r#"{"project_id":"project-a","repository":"repo"}"#,
    )
    .unwrap();
    assert_eq!(project_id(&top).as_deref(), Some("project-a"));
    assert!(read(&top).unwrap().unwrap().name.is_none());
    assert!(write(&top, "project-a", "repo", "Name").unwrap());
    assert_eq!(read(&top).unwrap().unwrap().name.as_deref(), Some("Name"));
    fs::write(top.join(FILE), "broken").unwrap();
    assert!(write(&top, "project-a", "repo", "Name").is_err());
    assert_eq!(fs::read_to_string(top.join(FILE)).unwrap(), "broken");
    fs::remove_dir_all(top).unwrap();
}
