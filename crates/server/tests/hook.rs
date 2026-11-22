//! Automated and subagent sessions do not get the Stop ask (docs/plan/0.1.9/12).

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
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let log = log.clone();
            thread::spawn(move || serve(stream, &log));
        }
    });
    Fake { url, seen }
}

fn serve(mut stream: std::net::TcpStream, log: &Mutex<Vec<Req>>) {
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
        r#"{"status":"stored"}"#
    } else if path.ends_with("/topic") {
        r#"{"notes":[]}"#
    } else if path.ends_with("/brief") {
        r#"{"brief":"BRIEF"}"#
    } else {
        r#"{"accepted":true,"duplicate":false,"event_id":"e","project_attachment":{"status":"personal"}}"#
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
        let call = json!({
            "tool_name": "Edit",
            "tool_input": {"file_path": "src/a.rs", "old_string": "a", "new_string": "x"},
            "tool_response": {"success": true},
            "tool_use_id": session
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
        reason.starts_with("You changed 1 file(s) but saved nothing to Cairn."),
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
            .starts_with("You changed 2 file(s) but saved nothing to Cairn."),
        "the parent still has its edits"
    );
}
