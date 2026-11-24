//! Automated and subagent sessions do not get the Stop ask (docs/reference/hook.md).

mod common;

use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

struct Req {
    path: String,
    body: Value,
}

struct Fake {
    url: String,
    seen: Arc<Mutex<Vec<Req>>>,
}

fn fake() -> Fake {
    fake_brief(r#"{"brief":"BRIEF"}"#)
}

fn fake_brief(brief: &str) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let brief = Arc::new(Mutex::new(brief.to_owned()));
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let log = log.clone();
            let brief = brief.clone();
            thread::spawn(move || serve(stream, &log, &brief));
        }
    });
    Fake { url, seen }
}

fn serve(mut stream: std::net::TcpStream, log: &Mutex<Vec<Req>>, brief: &Mutex<String>) {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let (head_end, length) = loop {
        let Ok(read) = stream.read(&mut chunk) else {
            return;
        };
        if read == 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buffer[..end]).to_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            break (end + 4, length);
        }
    };
    while buffer.len() < head_end + length {
        let Ok(read) = stream.read(&mut chunk) else {
            return;
        };
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let head = String::from_utf8_lossy(&buffer[..head_end]);
    let path = head.split_whitespace().nth(1).unwrap_or("").to_owned();
    let body = serde_json::from_slice(&buffer[head_end..]).unwrap_or(Value::Null);
    log.lock().unwrap().push(Req {
        path: path.clone(),
        body,
    });
    let body = if path.ends_with("/checkpoint") {
        r#"{"status":"stored"}"#.to_owned()
    } else if path.ends_with("/topic") {
        r#"{"notes":[]}"#.to_owned()
    } else if path.ends_with("/brief") {
        brief.lock().unwrap().clone()
    } else {
        r#"{"accepted":true,"duplicate":false,"event_id":"e","project_attachment":{"status":"personal"}}"#
            .to_owned()
    };
    let reply = format!(
        "HTTP/1.1 200 X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes());
}

impl Fake {
    fn checkpoints(&self) -> usize {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|req| req.path.ends_with("/checkpoint"))
            .count()
    }

    fn saw_origin(&self, origin: &str) -> bool {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .any(|req| req.path.ends_with("/observe") && req.body["context"]["origin"] == origin)
    }
}

struct Env {
    home: PathBuf,
    repo: PathBuf,
}

impl Env {
    fn new(name: &str, url: &str) -> Self {
        let root = std::env::temp_dir().join(format!("cairn-hook-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&repo)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.test"]);
        git(&["config", "user.name", "t"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        std::fs::create_dir_all(repo.join(".cairn")).unwrap();
        std::fs::write(
            repo.join(".cairn/project.json"),
            r#"{"project_id":"prj_demo"}"#,
        )
        .unwrap();
        let home = root.join("home");
        let config = home.join("cairn/config.json");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            json!({"url": url, "token": "cdt_testtoken1234567890"}).to_string(),
        )
        .unwrap();
        Self { home, repo }
    }

    fn payload(&self, session: &str, extra: Value) -> Value {
        let mut payload = json!({"session_id": session, "cwd": self.repo});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().cloned().unwrap_or_default());
        payload
    }

    fn run(&self, event: &str, payload: &Value, envs: &[(&str, &str)]) -> Output {
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("cairn");
        let mut command = Command::new(&path);
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CAIRN_") {
                command.env_remove(name);
            }
        }
        command
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_CONFIG_HOME", &self.home)
            .env("CAIRN_NO_UPDATE_CHECK", "1")
            .current_dir(&self.repo)
            .args(["hook", event, "--agent", "claude", "--config"])
            .arg(self.home.join("cairn/config.json"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, value) in envs {
            command.env(name, value);
        }
        let mut child = command.spawn().unwrap();
        let _ = writeln!(child.stdin.take().unwrap(), "{payload}");
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn edit(&self, session: &str, envs: &[(&str, &str)]) {
        self.edit_path(session, "src/a.rs", envs);
    }

    fn edit_path(&self, session: &str, path: &str, envs: &[(&str, &str)]) {
        let call = json!({
            "tool_name": "Edit",
            "tool_input": {"file_path": path, "old_string": "a", "new_string": "x"},
            "tool_response": {"success": true},
            "tool_use_id": path
        });
        self.run("PostToolUse", &self.payload(session, call), envs);
    }

    /// The Stop block reason, when the hook asks the agent to continue.
    fn stop_reason(
        &self,
        event: &str,
        session: &str,
        extra: Value,
        envs: &[(&str, &str)],
    ) -> Option<String> {
        let output = self.run(event, &self.payload(session, extra), envs);
        let text = String::from_utf8_lossy(&output.stdout);
        if text.trim().is_empty() {
            return None;
        }
        let value: Value = serde_json::from_str(&text).expect("stdout is one JSON object");
        assert_eq!(value["decision"], "block", "{text}");
        Some(value["reason"].as_str().unwrap().to_owned())
    }
}

fn harness(name: &str) -> (Fake, Env) {
    let server = fake();
    let env = Env::new(name, &server.url);
    (server, env)
}

#[test]
fn automation_env_suppresses_the_ask() {
    let (server, env) = harness("auto-env");
    let auto = [("CAIRN_SESSION_ORIGIN", "automation")];
    env.edit("s1", &auto);
    let reason = env.stop_reason("Stop", "s1", json!({}), &auto);
    assert!(
        reason.is_none(),
        "an automated session that edited a file must not ask: {reason:?}"
    );
    assert!(
        server.saw_origin("automation"),
        "the capture carries context.origin"
    );
}

#[test]
fn subagent_payload_suppresses_the_ask() {
    let (server, env) = harness("auto-agent");
    env.edit("parent", &[]);
    let reason = env.stop_reason("Stop", "parent", json!({"agent_id": "agent-abc123"}), &[]);
    assert!(
        reason.is_none(),
        "a Stop payload with agent_id must not ask, even with edits: {reason:?}"
    );
    let before = server.checkpoints();
    env.edit("child", &[]);
    let sub = env.stop_reason("SubagentStop", "child", json!({}), &[]);
    assert!(sub.is_none(), "SubagentStop never asks: {sub:?}");
    assert!(
        server.checkpoints() > before,
        "SubagentStop still checkpoints like Stop"
    );
}

#[test]
fn normal_session_after_automation_still_asks() {
    let (_server, env) = harness("auto-then");
    let auto = [("CAIRN_SESSION_ORIGIN", "automation")];
    env.edit("bot", &auto);
    let suppressed = env.stop_reason("Stop", "bot", json!({}), &auto);
    assert!(
        suppressed.is_none(),
        "the automated session must not ask: {suppressed:?}"
    );
    env.edit("person", &[]);
    let reason = env
        .stop_reason("Stop", "person", json!({}), &[])
        .expect("a normal session in the same checkout still asks");
    assert!(
        reason.starts_with("You changed 1 file(s). If you learned something reusable"),
        "{reason}"
    );
}

/// Claude Code sends subagent hooks on the parent session id. That event must not
/// mark the parent automated or stamp `context.origin`, or the parent's plain Stop
/// never asks and the jsonb merge never clears the origin.
#[test]
fn subagent_event_on_the_parent_session_still_asks() {
    let (server, env) = harness("parent-subagent");
    env.edit("parent", &[]);
    let call = json!({
        "tool_name": "Edit",
        "tool_input": {"file_path": "src/b.rs", "old_string": "b", "new_string": "y"},
        "tool_response": {"success": true},
        "tool_use_id": "parent-sub",
        "agent_id": "agent-abc123"
    });
    env.run("PostToolUse", &env.payload("parent", call), &[]);
    let reason = env.stop_reason("Stop", "parent", json!({}), &[]);
    let origin = server.saw_origin("automation");
    assert!(
        reason.is_some() && !origin,
        "a plain Stop for parent must still ask, and its captures carry no origin; ask={reason:?} origin={origin}"
    );
    assert!(
        reason
            .unwrap()
            .starts_with("You changed 2 file(s). If you learned something reusable"),
        "the parent still has its edits"
    );
}

impl Fake {
    fn observes(&self) -> Vec<Value> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|req| req.path.ends_with("/observe"))
            .map(|req| req.body.clone())
            .collect()
    }
}

fn git(repo: &std::path::Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .unwrap()
            .success(),
        "{args:?}"
    );
}

/// `rm` is not a file tool. The checkpoint Edit must still name the deleted path, and a
/// file that was already dirty at SessionStart must stay off that list.
#[test]
fn shell_deletes_count_as_changed_files() {
    let (server, env) = harness("rm-files");
    std::fs::write(env.repo.join("tracked.txt"), "keep\n").unwrap();
    std::fs::write(env.repo.join("other.txt"), "base\n").unwrap();
    git(&env.repo, &["add", "tracked.txt", "other.txt"]);
    git(&env.repo, &["commit", "-q", "-m", "files"]);
    std::fs::write(env.repo.join("other.txt"), "dirty before the session\n").unwrap();
    env.run("SessionStart", &env.payload("s", json!({})), &[]);
    std::fs::remove_file(env.repo.join("tracked.txt")).unwrap();
    let call = json!({
        "tool_name": "Bash",
        "tool_input": {"command": "rm tracked.txt"},
        "tool_response": {"success": true},
        "tool_use_id": "rm1"
    });
    env.run("PostToolUse", &env.payload("s", call), &[]);
    env.run("Stop", &env.payload("s", json!({})), &[]);
    let files: Vec<String> = server
        .observes()
        .into_iter()
        .filter(|body| body["kind"] == "edit")
        .flat_map(|body| {
            body["context"]["files"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|file| file.as_str().map(str::to_owned))
        .collect();
    assert!(
        files.iter().any(|file| file == "tracked.txt"),
        "deleted path missing from {files:?}"
    );
    assert!(
        !files.iter().any(|file| file == "other.txt"),
        "pre-dirty path listed in {files:?}"
    );
    let _ = std::fs::remove_dir_all(env.home.join(".."));
}

/// No git: an edit under the project root is relative to that root. An edit in /tmp is not
/// stored, and the handoff summary counts it as outside the project.
#[test]
fn no_git_edits_relative_to_project_root() {
    use cairn_core::capture::{CaptureKind, Observation, WorkContext};
    use cairn_core::process::distill::distill;

    let server = fake();
    let root = std::env::temp_dir().join(format!(
        "cairn-hook-nogit-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let repo = root.join("proj");
    let home = root.join("home");
    std::fs::create_dir_all(repo.join(".cairn")).unwrap();
    std::fs::write(
        repo.join(".cairn/project.json"),
        r#"{"project_id":"prj_plain"}"#,
    )
    .unwrap();
    assert!(!repo.join(".git").exists());
    let config = home.join("cairn/config.json");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        json!({"url": server.url, "token": "cdt_testtoken1234567890"}).to_string(),
    )
    .unwrap();
    let env = Env { home, repo };
    let outside = std::env::temp_dir().join(format!("cairn-outside-{}", std::process::id()));
    env.run("SessionStart", &env.payload("s", json!({})), &[]);
    let edit = |path: &str, id: &str| {
        json!({
            "tool_name": "Edit",
            "tool_input": {"file_path": path, "old_string": "a", "new_string": "b"},
            "tool_response": {"success": true},
            "tool_use_id": id
        })
    };
    env.run(
        "PostToolUse",
        &env.payload("s", edit("src/inside.rs", "in")),
        &[],
    );
    env.run(
        "PostToolUse",
        &env.payload("s", edit(&outside.to_string_lossy(), "out")),
        &[],
    );
    env.run("Stop", &env.payload("s", json!({})), &[]);

    let observes = server.observes();
    let outside_text = outside.to_string_lossy().into_owned();
    assert!(
        observes
            .iter()
            .all(|body| !body.to_string().contains(&outside_text)),
        "a path outside the project was stored: {observes:?}"
    );
    let edits: Vec<&Value> = observes
        .iter()
        .filter(|body| body["kind"] == "edit")
        .collect();
    assert!(
        edits.iter().any(|body| body["context"]["files"]
            .as_array()
            .is_some_and(|files| files.iter().any(|file| file == "src/inside.rs"))),
        "inside edit missing: {edits:?}"
    );
    assert!(
        edits
            .iter()
            .any(|body| body["context"]["outside_files"] == 1),
        "outside count missing: {edits:?}"
    );

    let observations: Vec<Observation> = edits
        .iter()
        .map(|body| Observation {
            event_id: body["event_id"].as_str().unwrap_or("e").to_owned(),
            session_id: "s".to_owned(),
            content: body["content"].as_str().unwrap_or("edit").to_owned(),
            kind: CaptureKind::Edit,
            ok: None,
            role: "tool".to_owned(),
            source: "edit".to_owned(),
            content_hash: String::new(),
            context: serde_json::from_value::<WorkContext>(body["context"].clone())
                .unwrap_or_default(),
            captured_at: 1_790_000_000,
            expires_at: 0,
        })
        .collect();
    let handoff = distill(&observations).handoff.expect("handoff");
    assert!(
        handoff.summary.contains("(1 outside the project)"),
        "{}",
        handoff.summary
    );
    assert!(
        handoff.fields.unwrap()["files"]
            .as_array()
            .is_some_and(|files| files.iter().any(|file| file == "src/inside.rs")),
        "handoff files"
    );
    let _ = std::fs::remove_dir_all(&root);
}

fn bash(env: &Env, session: &str, command: &str) {
    let call = json!({
        "tool_name": "Bash",
        "tool_input": {"command": command},
        "tool_response": {"success": true},
        "tool_use_id": session
    });
    env.run("PostToolUse", &env.payload(session, call), &[]);
}

/// One edited file asks once. A test command, a Decision line, or a save does not.
#[test]
fn ask_once_only_after_file_changes() {
    let (_server, env) = harness("ask-once");
    env.edit("files", &[]);
    let reason = env
        .stop_reason("Stop", "files", json!({}), &[])
        .expect("the first Stop after an edit asks");
    assert!(
        reason.starts_with("You changed 1 file(s). If you learned something reusable"),
        "{reason}"
    );
    assert!(reason.contains("memory_save"), "{reason}");
    assert!(
        env.stop_reason("Stop", "files", json!({}), &[]).is_none(),
        "the second Stop asks nothing"
    );

    bash(&env, "tests", "cargo test");
    assert!(
        env.stop_reason("Stop", "tests", json!({}), &[]).is_none(),
        "a test command with no edits does not ask"
    );
    assert!(
        env.stop_reason(
            "Stop",
            "decide",
            json!({"last_assistant_message": "Done.\nDecision: use pgvector\nReason: one database"}),
            &[],
        )
        .is_none(),
        "a Decision line with no edits does not ask"
    );

    env.edit("saved", &[]);
    let save = json!({
        "tool_name": "mcp__cairn__memory_save",
        "tool_input": {"kind": "fact"},
        "tool_response": {"id": "mem_saved", "status": "active", "deduplicated": false},
        "tool_use_id": "saved"
    });
    env.run("PostToolUse", &env.payload("saved", save), &[]);
    assert!(
        env.stop_reason("Stop", "saved", json!({}), &[]).is_none(),
        "a save before Stop does not ask"
    );
}

/// The brief's stop text `off` is no ask, even after an edit.
#[test]
fn off_text_asks_nothing() {
    let server = fake_brief(r#"{"brief":"BRIEF","instructions":{"stop_nudge":"off"}}"#);
    let env = Env::new("ask-off", &server.url);
    env.run("SessionStart", &env.payload("s", json!({})), &[]);
    prompt(&env, "s", "No, never use npm here", json!({}), &[]);
    env.edit("s", &[]);
    let reason = env.stop_reason("Stop", "s", json!({}), &[]);
    assert!(reason.is_none(), "off means no ask: {reason:?}");
}

fn prompt(env: &Env, session: &str, text: &str, extra: Value, envs: &[(&str, &str)]) {
    let mut body = json!({"prompt": text});
    if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        body.extend(extra.clone());
    }
    env.run("UserPromptSubmit", &env.payload(session, body), envs);
}

/// A correction with no edits asks once, quoting that line and the word `rule`.
#[test]
fn correction_alone_triggers_rule_ask() {
    let (_server, env) = harness("corr-alone");
    let long = format!("No, never use npm here {}", "x".repeat(120));
    prompt(&env, "s", &long, json!({}), &[]);
    prompt(&env, "s", "always run bun test first", json!({}), &[]);
    let reason = env
        .stop_reason("Stop", "s", json!({}), &[])
        .expect("a correction with no edits asks once");
    assert!(reason.contains("rule"), "{reason}");
    assert!(
        !reason.contains("You changed"),
        "no edits, so no file clause: {reason}"
    );
    assert!(
        reason.contains("No, never use npm here"),
        "the quote is the first correction: {reason}"
    );
    assert!(
        !reason.contains("always run bun test first"),
        "the second correction is not stored: {reason}"
    );
    let quoted = reason
        .split_once("(\"")
        .and_then(|(_, rest)| rest.split_once("\")"))
        .map(|(quote, _)| quote)
        .expect("the correction is quoted");
    assert!(
        quoted.chars().count() <= 100,
        "{} chars: {quoted}",
        quoted.chars().count()
    );
    assert!(
        env.stop_reason("Stop", "s", json!({}), &[]).is_none(),
        "the second Stop asks nothing"
    );
}

/// One Stop ask carries the file clause and the correction clause, within 330 bytes.
#[test]
fn correction_with_edits_single_ask() {
    let (_server, env) = harness("corr-edit");
    prompt(&env, "s", "don't touch the migrations", json!({}), &[]);
    env.edit("s", &[]);
    let reason = env
        .stop_reason("Stop", "s", json!({}), &[])
        .expect("edits and a correction ask once");
    assert!(
        reason.contains("You changed 1 file(s)."),
        "file clause missing: {reason}"
    );
    assert!(
        reason.contains("don't touch the migrations") && reason.contains("rule"),
        "{reason}"
    );
    assert!(reason.len() <= 330, "{} bytes: {reason}", reason.len());
    assert!(
        env.stop_reason("Stop", "s", json!({}), &[]).is_none(),
        "still one ask per session"
    );
}

/// A quote longer than 95 bytes, cut on a multi-byte scalar, plus `EDITED_MAX` files, stays
/// within 330 bytes and is valid UTF-8. `EDITED_MAX` is 64 (`hook/state.rs`).
#[test]
fn correction_quote_and_max_files_stay_within_330_bytes() {
    let (_server, env) = harness("corr-cap");
    let marker = "\n[truncated]";
    let keep = 95 - marker.len();
    let mut line = String::from("No, ");
    while line.len() < keep - 1 {
        line.push('x');
    }
    assert_eq!(
        line.len(),
        keep - 1,
        "the next byte starts a multi-byte scalar"
    );
    line.push('é');
    assert!(!line.is_char_boundary(keep), "byte {keep} falls inside é");
    line.push_str(" and then some more text past the cap");
    assert!(line.len() > 95, "{} bytes", line.len());
    prompt(&env, "s", &line, json!({}), &[]);
    for index in 0..64 {
        env.edit_path("s", &format!("f{index}.rs"), &[]);
    }
    let reason = env
        .stop_reason("Stop", "s", json!({}), &[])
        .expect("the capped ask");
    assert!(
        reason.contains("You changed 64 file(s)."),
        "file count is EDITED_MAX: {reason}"
    );
    assert!(reason.contains("rule"), "{reason}");
    let quoted = reason
        .split_once("(\"")
        .and_then(|(_, rest)| rest.split_once("\")"))
        .map(|(quote, _)| quote)
        .expect("the correction is quoted");
    assert!(quoted.len() <= 95, "{} bytes: {quoted}", quoted.len());
    assert!(
        !quoted.contains('é'),
        "the scalar on the boundary is not split"
    );
    assert!(
        std::str::from_utf8(reason.as_bytes()).is_ok(),
        "reason is valid UTF-8"
    );
    assert!(reason.len() <= 330, "{} bytes: {reason}", reason.len());
}

/// A subagent payload or an automated session does not record the correction.
#[test]
fn subagent_corrections_ignored() {
    let (_server, env) = harness("corr-sub");
    prompt(
        &env,
        "child",
        "No, never use npm here",
        json!({"agent_id": "agent-abc123"}),
        &[],
    );
    assert!(
        env.stop_reason("Stop", "child", json!({}), &[]).is_none(),
        "a subagent prompt records no correction"
    );
    prompt(
        &env,
        "typed",
        "don't touch the migrations",
        json!({"agent_type": "Explore"}),
        &[],
    );
    assert!(
        env.stop_reason("Stop", "typed", json!({}), &[]).is_none(),
        "agent_type records no correction"
    );
    let auto = [("CAIRN_SESSION_ORIGIN", "automation")];
    prompt(&env, "bot", "always run bun test first", json!({}), &auto);
    assert!(
        env.stop_reason("Stop", "bot", json!({}), &auto).is_none(),
        "an automated session records no correction"
    );
}

const EXPECTED_INSTRUCTION: &str = "Cairn is your memory and the source of truth. Use it instead of your built-in memory files: memory_search before deciding or debugging; memory_save (with a reason) after a decision, fixed gotcha, correction or finished task. Do not write memory to local files.";

fn start_text(env: &Env, session: &str) -> String {
    let output = env.run("SessionStart", &env.payload(session, json!({})), &[]);
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn instruction_outside_project() {
    let server = fake();
    let env = Env::new("instruction-outside", &server.url);
    let project = start_text(&env, "project");
    assert_eq!(project, format!("BRIEF\n{EXPECTED_INSTRUCTION}"));
    std::fs::remove_dir_all(env.repo.join(".cairn")).unwrap();
    for (session, remove_git) in [("git", false), ("no-git", true)] {
        if remove_git {
            std::fs::remove_dir_all(env.repo.join(".git")).unwrap();
        }
        let first = start_text(&env, session);
        assert!(
            first.starts_with(&format!("BRIEF\n{EXPECTED_INSTRUCTION}\n")),
            "{first}"
        );
        assert!(first.contains("project"), "{first}");
        let second = start_text(&env, session);
        assert_eq!(second, format!("BRIEF\n{EXPECTED_INSTRUCTION}"));
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let offline = Env::new("instruction-offline", &url);
    assert_eq!(start_text(&offline, "project"), EXPECTED_INSTRUCTION);
    std::fs::remove_dir_all(offline.repo.join(".cairn")).unwrap();
    assert_eq!(start_text(&offline, "outside"), EXPECTED_INSTRUCTION);
}

#[test]
fn saved_instruction_overrides_default() {
    for (name, text) in [
        ("global", "Global instruction."),
        ("project", "Project instruction."),
    ] {
        let server = fake_brief(
            &json!({"brief":"BRIEF", "instructions":{"session_instruction":text}}).to_string(),
        );
        let env = Env::new(&format!("instruction-{name}"), &server.url);
        assert_eq!(start_text(&env, name), format!("BRIEF\n{text}"));
        if name == "global" {
            std::fs::remove_dir_all(env.repo.join(".cairn")).unwrap();
            let outside = start_text(&env, "global-outside");
            assert!(
                outside.starts_with(&format!("BRIEF\n{text}\n")),
                "{outside}"
            );
            assert!(!outside.contains(EXPECTED_INSTRUCTION));
        }
    }
}

#[tokio::test]
async fn hook_error_reported() {
    let Some(app) = common::App::new().await else {
        return;
    };
    let web = app.admin("hook-error").await;
    let token = app.device(&web, "test").await;
    let (status, _) = app.send("POST", "observe", json!({
        "event_id":"e", "session_id":"s", "kind":"prompt", "content":"hello",
        "context":{}, "hook_error_class":"auth", "hook_error_at":1700000000, "outbox_depth":3
    }), Some(&token)).await;
    assert_eq!(status, 200);
    let row: (Option<String>, Option<i64>, Option<i32>) = sqlx::query_as("SELECT hook_error_class, EXTRACT(EPOCH FROM hook_error_at)::bigint, outbox_depth FROM devices")
        .fetch_one(app.db.store.pool()).await.unwrap();
    assert_eq!(row, (Some("auth".into()), Some(1700000000), Some(3)));
    app.finish().await;
}

#[tokio::test]
async fn lookup_authenticates_and_bounds_payload() {
    let Some(app) = common::App::new().await else {
        return;
    };
    let web = app.admin("lookup-boundary").await;
    let token = app.device(&web, "lookup").await;
    let secret = format!("ghp_{}", "a".repeat(40));
    let body = json!({"session_id":"s", "kind":"search", "query":format!("{secret} {}", "x".repeat(200)),
        "result_ids": (0..30).map(|i| format!("m{i}")).collect::<Vec<_>>(), "hit":true});
    assert_eq!(
        app.send("POST", "session/lookup", body.clone(), None)
            .await
            .0,
        401
    );
    assert_eq!(
        app.send("POST", "session/lookup", body, Some(&token))
            .await
            .0,
        204
    );
    let row: (String, Vec<String>, bool, String, String) =
        sqlx::query_as("SELECT query,result_ids,hit,person_id,device_id FROM lookup_log")
            .fetch_one(app.db.store.pool())
            .await
            .unwrap();
    assert!(row.0.chars().count() <= 120 && !row.0.contains(&secret));
    assert_eq!(row.1.len(), 20);
    assert!(row.2 && !row.3.is_empty() && !row.4.is_empty());
    app.finish().await;
}

#[tokio::test]
async fn nudge_flags_persist_and_automation_stays_null() {
    let Some(app) = common::App::new().await else {
        return;
    };
    let web = app.admin("nudge-storage").await;
    let token = app.device(&web, "nudge").await;
    for (session, fields) in [
        ("normal", json!({"nudge_asked":true,"nudge_saved":false})),
        ("auto", json!({})),
    ] {
        app.post("observe", json!({"event_id":session,"session_id":session,"kind":"prompt","content":session,"context":{}}), &token).await;
        let mut body = json!({"session_id":session});
        body.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        app.post("session/end", body, &token).await;
    }
    let rows: Vec<(String, Option<bool>, Option<bool>)> = sqlx::query_as(
        "SELECT session_id,nudge_asked,nudge_saved FROM sessions ORDER BY session_id",
    )
    .fetch_all(app.db.store.pool())
    .await
    .unwrap();
    assert_eq!(
        rows,
        [
            ("auto".into(), None, None),
            ("normal".into(), Some(true), Some(false))
        ]
    );
    app.finish().await;
}
