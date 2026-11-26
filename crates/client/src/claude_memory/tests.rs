use super::*;

#[test]
fn project_names_follow_claude_encoding() {
    assert_eq!(
        project_folder("/home/a_b/repo.with spaces/😀"),
        "-home-a-b-repo-with-spaces---"
    );
    assert_eq!(
        project_folder(&format!("/{}😀", "a".repeat(215))),
        format!("-{}-fz2wq3", "a".repeat(199))
    );
    assert!(valid_project_dir_name("cairn_project-2"));
    for invalid in ["../outside", ".", "", "café", "CON", "com1", "LpT9"] {
        assert!(!valid_project_dir_name(invalid), "{invalid}");
    }
}

#[test]
fn non_initialized_project_memory_stays_personal() {
    let raw = "---\r\nname: Working decisions\r\ndescription: >-\r\n  Keep decisions scoped\r\n  to the relevant workspace.\r\nmetadata: { type: project }\r\n---\r\n";
    let ImportRecord {
        args: record,
        clipped,
    } = import_record(raw, "/tmp/note.md", false).unwrap().unwrap();
    assert_eq!(record["kind"], "note");
    assert_eq!(record["scope"], "personal");
    assert_eq!(
        record["summary"],
        "Keep decisions scoped to the relevant workspace."
    );
    assert_eq!(record["reason"], "Imported from Claude memory");
    assert!(!clipped);
}
