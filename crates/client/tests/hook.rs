//! `cairn hook <Event>`: the real binary against a scripted fake server (transport, queueing,
//! injection, state) and against a real server on a `TestDb` schema (typed `/observe` bodies,
//! topic notes). No test needs Python.

mod common;

use cairn_client::Config;
use common::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const TOKEN: &str = "cdt_testtoken1234567890";

#[derive(Clone)]
struct Req {
    path: String,
    auth: String,
    body: Value,
}

type Responder = Arc<dyn Fn(usize, &Req) -> (u16, String) + Send + Sync>;

/// A scripted server on a random loopback port; every request is recorded. A 3xx answer sends
/// the body as the `location` header.
struct Fake {
    url: String,
    seen: Arc<Mutex<Vec<Req>>>,
}

fn default_reply(request: &Req) -> (u16, String) {
    let body = match request.path.rsplit('/').next().unwrap_or("") {
        "observe" => {
            json!({"accepted": true, "duplicate": false, "event_id": request.body["event_id"],
                            "project_attachment": {"status": "personal"}})
        }
        "brief" => json!({"brief": "BRIEF"}),
        "topic" => json!({"notes": []}),
        "checkpoint" => json!({"status": "stored"}),
        _ => json!({"status": "queued"}),
    };
    (200, body.to_string())
}

fn fake_with(responder: impl Fn(usize, &Req) -> (u16, String) + Send + Sync + 'static) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen: Arc<Mutex<Vec<Req>>> = Arc::default();
    let responder: Responder = Arc::new(responder);
    let log = seen.clone();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (log, responder) = (log.clone(), responder.clone());
            thread::spawn(move || serve(stream, &log, &responder));
        }
    });
    Fake { url, seen }
}

fn serve(mut stream: std::net::TcpStream, log: &Mutex<Vec<Req>>, responder: &Responder) {
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
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let request = Req {
        path: head.split_whitespace().nth(1).unwrap_or("").to_owned(),
        auth: head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("authorization")
                    .then(|| value.trim().to_owned())
            })
            .unwrap_or_default(),
        body: serde_json::from_slice(&buffer[head_end..]).unwrap_or(Value::Null),
    };
    let index = {
        let mut log = log.lock().unwrap();
        log.push(request.clone());
        log.len() - 1
    };
    let (status, body) = responder(index, &request);
    let reply = if (300..400).contains(&status) {
        format!(
            "HTTP/1.1 {status} Redirect\r\nlocation: {body}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
        )
    } else {
        format!(
            "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    };
    let _ = stream.write_all(reply.as_bytes());
}

fn fake() -> Fake {
    fake_with(|_, request| default_reply(request))
}

impl Fake {
    fn requests(&self) -> Vec<Req> {
        self.seen.lock().unwrap().clone()
    }

    fn to(&self, name: &str) -> Vec<Req> {
        self.requests()
            .into_iter()
            .filter(|r| r.path.ends_with(&format!("/{name}")))
            .collect()
    }

    fn observed(&self) -> Vec<Value> {
        self.to("observe").into_iter().map(|r| r.body).collect()
    }
}

/// A config home, a git checkout, and the config file pointing at `url`.
struct Env {
    home: PathBuf,
    repo: PathBuf,
}

impl Env {
    fn new(name: &str, url: &str) -> Self {
        let repo = checkout(name);
        let home = repo.parent().unwrap().join("home");
        Config::new(url, TOKEN)
            .unwrap()
            .save(&home.join("cairn").join("config.json"))
            .unwrap();
        Self { home, repo }
    }

    fn config(&self) -> PathBuf {
        self.home.join("cairn").join("config.json")
    }

    fn state_dir(&self) -> PathBuf {
        self.home.join("cairn").join("hook-state")
    }

    fn outbox(&self) -> PathBuf {
        self.home.join("cairn").join("outbox.json")
    }

    fn payload(&self, session: &str, extra: Value) -> Value {
        let mut payload = json!({"session_id": session, "cwd": self.repo});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        payload
    }

    fn run_with(
        &self,
        event: &str,
        payload: &Value,
        extra: &[&str],
        envs: &[(&str, &str)],
    ) -> Output {
        let config = self.config();
        let mut command = command(&self.home);
        command
            .current_dir(&self.repo)
            .args(["hook", event, "--agent", "claude", "--config"])
            .arg(&config);
        command.args(extra);
        for (name, value) in envs {
            command.env(name, value);
        }
        let output = with_stdin(&mut command, &payload.to_string());
        assert_eq!(
            output.status.code(),
            Some(0),
            "a hook always exits 0: {}",
            stderr(&output)
        );
        output
    }

    fn run(&self, event: &str, payload: &Value) -> Output {
        self.run_with(event, payload, &[], &[])
    }

    fn prompt(&self, session: &str, text: &str) -> Output {
        self.run(
            "UserPromptSubmit",
            &self.payload(session, json!({"prompt": text})),
        )
    }

    fn edit(&self, session: &str, path: &str, text: &str) -> Output {
        self.run(
            "PostToolUse",
            &self.payload(session, edit_call(path, text, path)),
        )
    }

    fn bash(&self, session: &str, command: &str, response: Value) -> Output {
        let call = json!({"tool_name": "Bash", "tool_input": {"command": command}, "tool_response": response,
                          "tool_use_id": command});
        self.run("PostToolUse", &self.payload(session, call))
    }
}

fn edit_call(path: &str, text: &str, tool_use_id: &str) -> Value {
    json!({"tool_name": "Edit", "tool_input": {"file_path": path, "old_string": "a", "new_string": text},
           "tool_response": {"success": true}, "tool_use_id": tool_use_id})
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn injected(output: &Output) -> Option<String> {
    let text = String::from_utf8_lossy(&output.stdout);
    if text.trim().is_empty() {
        return None;
    }
    let value: Value = serde_json::from_str(&text).expect("stdout is one JSON object");
    Some(
        value["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .to_owned(),
    )
}

fn notes_reply(request: &Req) -> (u16, String) {
    if request.path.ends_with("/topic") {
        let path = request.body["path"].as_str().unwrap();
        let notes = if path.contains("quiet") {
            json!([])
        } else {
            json!([{"id": "m1", "title": format!("Note for {path}"), "summary": "s".repeat(500)}])
        };
        return (200, json!({"notes": notes}).to_string());
    }
    default_reply(request)
}

fn device_of(token: &str) -> String {
    format!("{:x}", Sha256::digest(format!("cairn-outbox-v3\0{token}")))[..32].to_owned()
}

fn write_outbox(path: &Path, origin: &str, token: &str, ids: &[String]) {
    let events: Vec<Value> = ids
        .iter()
        .map(|id| {
            json!({"event_id": id, "attempts": 0, "last_error": null,
                   "body": {"event_id": id, "session_id": "old", "kind": "prompt", "content": format!("queued {id}"), "context": {}}})
        })
        .collect();
    let queue =
        json!({"version": 3, "origin": origin, "device": device_of(token), "events": events});
    std::fs::write(path, queue.to_string()).unwrap();
}

fn queued_ids(path: &Path) -> Vec<String> {
    let queue: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    queue["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event_id"].as_str().unwrap().to_owned())
        .collect()
}

// ---- events -----------------------------------------------------------------------------

#[test]
fn start_injects_brief_only_and_end_after_work() {
    let server = fake();
    let env = Env::new("hook-start", &server.url);
    let start = env.run(
        "SessionStart",
        &env.payload("s1", json!({"source": "startup"})),
    );
    assert_eq!(injected(&start).as_deref(), Some("BRIEF"));
    let value: Value = serde_json::from_slice(&start.stdout).unwrap();
    assert_eq!(value["hookSpecificOutput"]["hookEventName"], "SessionStart");
    let brief = &server.to("brief")[0];
    assert_eq!(brief.auth, format!("Bearer {TOKEN}"));
    assert_eq!(brief.body["session_id"], "s1");
    assert_eq!(brief.body["context"]["repository"], "local:repo");
    assert_eq!(brief.body["recent_files"], json!([]));

    // No work: SessionEnd closes nothing on the server and removes the state.
    env.run("SessionEnd", &env.payload("s1", json!({})));
    assert!(server.to("end").is_empty());

    env.edit("s2", "src/a.rs", "fn a() {}");
    assert!(!injected(&env.run("SessionEnd", &env.payload("s2", json!({})))).is_some());
    assert_eq!(server.to("end").len(), 1);
    assert_eq!(server.to("end")[0].body["session_id"], "s2");
    let left = std::fs::read_dir(env.state_dir())
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(left, 0, "state files are deleted at SessionEnd");
}

#[test]
fn recent_files_list_status_then_commits_without_duplicates() {
    let server = fake();
    let env = Env::new("hook-recent", &server.url);
    std::fs::write(env.repo.join("new.rs"), "x").unwrap();
    env.run(
        "SessionStart",
        &env.payload("s1", json!({"source": "resume"})),
    );
    assert_eq!(
        server.to("brief")[0].body["recent_files"],
        json!(["new.rs"])
    );
}

#[test]
fn stop_keeps_assistant_opt_in() {
    let server = fake();
    let env = Env::new("hook-assistant", &server.url);
    let stop = env.payload(
        "s1",
        json!({"last_assistant_message": "I changed the lock order."}),
    );
    env.run("Stop", &stop);
    assert!(server.observed().is_empty());
    env.run_with("Stop", &stop, &[], &[("CAIRN_CAPTURE_ASSISTANT", "1")]);
    let body = &server.observed()[0];
    assert_eq!(
        (body["kind"].as_str(), body["content"].as_str()),
        (Some("assistant"), Some("I changed the lock order."))
    );
}

#[test]
fn config_precedence() {
    let (one, two) = (fake(), fake());
    let env = Env::new("hook-config", &one.url);
    let other = env.home.join("other.json");
    Config::new(&two.url, TOKEN).unwrap().save(&other).unwrap();
    let payload = env.payload("s1", json!({"prompt": "hello"}));
    let other_path = other.to_str().unwrap();

    // Unmanaged: --config wins over CAIRN_CONFIG.
    env.run_with(
        "UserPromptSubmit",
        &payload,
        &[],
        &[("CAIRN_CONFIG", other_path)],
    );
    assert_eq!((one.observed().len(), two.observed().len()), (1, 0));
    // Managed: a non-empty CAIRN_CONFIG replaces the setup-time path.
    let payload = env.payload("s1", json!({"prompt": "second"}));
    env.run_with(
        "UserPromptSubmit",
        &payload,
        &["--cairn-managed"],
        &[("CAIRN_CONFIG", other_path)],
    );
    assert_eq!((one.observed().len(), two.observed().len()), (1, 1));
    // Managed with an empty CAIRN_CONFIG keeps --config.
    let payload = env.payload("s1", json!({"prompt": "third"}));
    env.run_with(
        "UserPromptSubmit",
        &payload,
        &["--cairn-managed"],
        &[("CAIRN_CONFIG", "  ")],
    );
    assert_eq!(one.observed().len(), 2);
    // The legacy variables are ignored, with one stderr note, and never sent.
    let payload = env.payload("s1", json!({"prompt": "fourth"}));
    let output = env.run_with(
        "UserPromptSubmit",
        &payload,
        &[],
        &[
            ("CAIRN_URL", "http://127.0.0.1:9"),
            ("CAIRN_DEVICE_TOKEN", "cdt_other"),
        ],
    );
    assert!(
        stderr(&output).contains("ignoring CAIRN_URL and CAIRN_DEVICE_TOKEN"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        one.requests().last().unwrap().auth,
        format!("Bearer {TOKEN}")
    );
}

#[test]
fn a_missing_config_turns_the_hook_off() {
    let server = fake();
    let env = Env::new("hook-noconfig", &server.url);
    std::fs::remove_file(env.config()).unwrap();
    let output = env.prompt("s1", "hello");
    assert!(
        output.stdout.is_empty() && output.stderr.is_empty(),
        "{}",
        stderr(&output)
    );
    assert!(server.requests().is_empty() && !env.state_dir().exists());
}

#[test]
fn many_sessions_keep_state_without_payload_or_token() {
    let server = fake();
    let env = Env::new("hook-many", &server.url);
    for index in 0..6 {
        env.edit(
            &format!("session-{index}"),
            "src/a.rs",
            &format!("private body {index}"),
        );
    }
    let files: Vec<_> = std::fs::read_dir(env.state_dir())
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .collect();
    assert_eq!(files.len(), 6);
    for file in files {
        let text = std::fs::read_to_string(file.path()).unwrap();
        for secret in [
            TOKEN,
            "private body",
            "session-",
            env.repo.to_str().unwrap(),
        ] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
    }
}

#[test]
fn http_401_names_config_keeps_stdout_valid() {
    let server = fake_with(|_, _| {
        (
            401,
            json!({"error": format!("bad token {TOKEN}")}).to_string(),
        )
    });
    let env = Env::new("hook-401", &server.url);
    let start = env.run("SessionStart", &env.payload("s1", json!({})));
    let prompt = env.prompt("s1", "hello");
    for output in [&start, &prompt] {
        let text = stderr(output);
        assert!(output.stdout.is_empty());
        assert!(text.contains(env.config().to_str().unwrap()), "{text}");
        assert!(text.contains("cairn setup"), "{text}");
        assert!(!text.contains(TOKEN), "{text}");
    }
    assert!(!env.outbox().exists(), "a 401 is not queued");
}

#[test]
fn cairnignore_disables_everything() {
    let server = fake();
    let env = Env::new("hook-ignore", &server.url);
    std::fs::write(env.repo.join(".cairnignore"), "").unwrap();
    let sub = env.repo.join("deep");
    std::fs::create_dir_all(&sub).unwrap();
    for cwd in [&env.repo, &sub] {
        env.run(
            "UserPromptSubmit",
            &json!({"session_id": "s1", "cwd": cwd, "prompt": "hello"}),
        );
    }
    // No cwd in the payload: the process folder counts.
    env.run(
        "UserPromptSubmit",
        &json!({"session_id": "s1", "prompt": "hello"}),
    );
    assert!(server.requests().is_empty() && !env.state_dir().exists() && !env.outbox().exists());
    // A marker above the git top does not apply to the checkout.
    std::fs::remove_file(env.repo.join(".cairnignore")).unwrap();
    std::fs::write(env.repo.parent().unwrap().join(".cairnignore"), "").unwrap();
    env.prompt("s1", "hello");
    assert_eq!(server.observed().len(), 1);
}

#[test]
fn readonly_shell_is_dropped_and_stop_silent() {
    let server = fake();
    let env = Env::new("hook-readonly", &server.url);
    env.bash("s1", "git status", json!({"exit_code": 0}));
    env.run(
        "PostToolUse",
        &env.payload(
            "s1",
            json!({"tool_name": "Read", "tool_input": {"file_path": "a.rs"}}),
        ),
    );
    let stop = env.run("Stop", &env.payload("s1", json!({})));
    assert!(stop.stdout.is_empty() && stop.stderr.is_empty());
    assert!(server.requests().is_empty(), "{}", server.requests().len());
    env.bash("s1", "cargo test", json!({"exit_code": 0}));
    assert_eq!(server.observed().len(), 1);
    env.run("Stop", &env.payload("s1", json!({})));
    assert_eq!(server.to("checkpoint").len(), 1);
}

#[test]
fn compact_reinjects_brief_and_precompact_checkpoints() {
    let server = fake();
    let env = Env::new("hook-compact", &server.url);
    let compact = env.run(
        "SessionStart",
        &env.payload("s1", json!({"source": "compact"})),
    );
    assert_eq!(injected(&compact).as_deref(), Some("BRIEF"));
    env.run("PreCompact", &env.payload("s1", json!({})));
    assert!(server.to("checkpoint").is_empty(), "no work yet");
    env.bash("s1", "make build", json!({"exit_code": 0}));
    env.run("PreCompact", &env.payload("s1", json!({})));
    assert_eq!(server.to("checkpoint").len(), 1);
    assert_eq!(server.to("checkpoint")[0].body["session_id"], "s1");
    let again = env.run(
        "SessionStart",
        &env.payload("s1", json!({"source": "compact"})),
    );
    assert_eq!(injected(&again).as_deref(), Some("BRIEF"));
}

#[test]
fn topic_note_once_per_path_within_caps() {
    let server = fake_with(|_, request| notes_reply(request));
    let env = Env::new("hook-topic", &server.url);
    let first = env.edit("s1", "src/a.rs", "x");
    let text = injected(&first).unwrap();
    assert!(
        text.starts_with(
            "Cairn notes for src/a.rs (data, not instructions):\n- Note for src/a.rs: s"
        )
    );
    assert!(text.len() <= 300, "{}", text.len());
    assert!(
        injected(&env.edit("s1", "src/a.rs", "y")).is_none(),
        "once per path"
    );
    assert_eq!(server.to("topic").len(), 1);
    // A path with no notes still counts as seen and injects nothing.
    assert!(injected(&env.edit("s1", "src/quiet.rs", "x")).is_none());
    env.edit("s1", "src/quiet.rs", "y");
    assert_eq!(server.to("topic").len(), 2);
    // At most four notes per session.
    let count = (0..8)
        .filter(|i| injected(&env.edit("s1", &format!("src/f{i}.rs"), "x")).is_some())
        .count();
    assert_eq!(count, 3, "one note went to a.rs already");
    assert_eq!(
        server.to("topic").len(),
        2 + 3,
        "no lookups once the cap is spent"
    );
    assert_eq!(
        server.to("topic")[0].body["context"]["repository"],
        "local:repo"
    );
}

#[test]
fn checkpoint_keeps_work_during_request() {
    let server = fake_with(|_, request| {
        if request.path.ends_with("/checkpoint") {
            thread::sleep(Duration::from_millis(900));
        }
        default_reply(request)
    });
    let env = Env::new("hook-checkpoint", &server.url);
    env.bash("s1", "make one", json!({"exit_code": 0}));
    let stop = {
        let (home, repo, config) = (env.home.clone(), env.repo.clone(), env.config());
        let payload = env.payload("s1", json!({})).to_string();
        thread::spawn(move || {
            let mut command = command(&home);
            command
                .current_dir(&repo)
                .args(["hook", "Stop", "--agent", "claude", "--config"])
                .arg(&config);
            with_stdin(&mut command, &payload)
        })
    };
    thread::sleep(Duration::from_millis(350));
    env.bash("s1", "make two", json!({"exit_code": 0}));
    assert!(stop.join().unwrap().status.success());
    assert_eq!(server.to("checkpoint").len(), 1);
    // The work that arrived during the request is still pending.
    env.run("Stop", &env.payload("s1", json!({})));
    assert_eq!(server.to("checkpoint").len(), 2);
    env.run("Stop", &env.payload("s1", json!({})));
    assert_eq!(server.to("checkpoint").len(), 2, "now clean");
}

#[test]
fn worktree_context_inside_one_checkout() {
    let server = fake();
    let env = Env::new("hook-context", &server.url);
    let sub = env.repo.join("pkg");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::create_dir_all(env.repo.join(".cairn")).unwrap();
    std::fs::write(
        env.repo.join(".cairn/project.json"),
        r#"{"project_id":"prj_demo"}"#,
    )
    .unwrap();
    let prompt = |cwd: &Path, text: &str| {
        env.run(
            "UserPromptSubmit",
            &json!({"session_id": "s1", "cwd": cwd, "prompt": text}),
        )
    };
    prompt(&env.repo, "one");
    prompt(&sub, "two");
    let other = checkout("hook-context-other");
    prompt(&other, "three");
    git(&other, &["checkout", "-q", "--detach"]);
    prompt(&other, "four");
    let plain = temp_dir("hook-context-plain");
    prompt(&plain, "five");
    let contexts: Vec<Value> = server
        .observed()
        .iter()
        .map(|b| b["context"].clone())
        .collect();
    assert_eq!(contexts[0]["repository"], "local:repo");
    assert_eq!(contexts[0]["branch"], "main");
    assert_eq!(contexts[0]["project_id"], "prj_demo");
    assert_eq!(
        contexts[0]["worktree"], contexts[1]["worktree"],
        "same checkout"
    );
    assert!(contexts[0]["worktree"].as_str().unwrap().starts_with("wt-"));
    assert_ne!(contexts[0]["worktree"], contexts[2]["worktree"]);
    assert_eq!(contexts[2]["project_id"], Value::Null);
    assert_eq!(
        contexts[3]["branch"],
        Value::Null,
        "detached HEAD has no branch"
    );
    assert_eq!(contexts[3]["repository"], "local:repo");
    assert_eq!(contexts[4]["repository"], Value::Null, "not a repository");
}

fn git(dir: &Path, args: &[&str]) {
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn payload_maps_to_typed_capture() {
    let server = fake();
    let env = Env::new("hook-typed", &server.url);
    env.prompt("s1", "please fix the queue");
    env.edit("s1", "src/queue.rs", "fn lock() {}");
    env.bash("s1", "cargo test -q", json!({"exit_code": 101, "stderr": "\nerror[E0432]: unresolved import\nnote", "stdout": ""}));
    env.bash("s1", "cargo build", json!({"exit_code": 0}));
    let bodies = server.observed();
    assert_eq!(bodies.len(), 4);
    for body in &bodies {
        let mut keys: Vec<&str> = body
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert!(
            keys.iter().all(
                |k| ["content", "context", "event_id", "kind", "ok", "session_id"].contains(k)
            ),
            "{keys:?}"
        );
        assert!(body["event_id"].as_str().unwrap().starts_with("hk_"));
    }
    assert_eq!(bodies[0]["kind"], "prompt");
    assert_eq!(bodies[0].get("ok"), None);
    assert_eq!(bodies[1]["kind"], "edit");
    assert_eq!(bodies[1]["content"], "Edit src/queue.rs\nfn lock() {}");
    assert_eq!(bodies[1]["context"]["files"], json!(["src/queue.rs"]));
    assert_eq!(bodies[2]["kind"], "command");
    assert_eq!(bodies[2]["ok"], false);
    assert_eq!(
        bodies[2]["content"],
        "cargo test -q\nexit 101: error[E0432]: unresolved import"
    );
    assert_eq!(bodies[2]["context"]["commands"], json!(["cargo test -q"]));
    assert_eq!(bodies[3]["ok"], true);
    // A repeat in the same session is not sent again.
    env.bash("s1", "cargo build", json!({"exit_code": 0}));
    assert_eq!(server.observed().len(), 4);
}

#[test]
fn capture_is_redacted_then_clipped() {
    let server = fake();
    let env = Env::new("hook-redact", &server.url);
    let secret = format!("ghp_{}", "a".repeat(40));
    env.prompt(
        "s1",
        &format!(
            "use token={secret} and password: hunter2 {}",
            "x".repeat(20_000)
        ),
    );
    env.bash(
        "s1",
        &format!("curl -H 'Authorization: Bearer {secret}' https://x.test --password hunter2"),
        json!({"exit_code": 0}),
    );
    let bodies = server.observed();
    let prompt = bodies[0]["content"].as_str().unwrap();
    assert!(!prompt.contains(&secret) && !prompt.contains("hunter2"));
    assert!(
        prompt.len() <= 8 * 1024 && prompt.ends_with("[truncated]"),
        "{}",
        prompt.len()
    );
    let command = bodies[1]["content"].as_str().unwrap();
    assert!(
        !command.contains(&secret) && !command.contains("hunter2"),
        "{command}"
    );
    assert!(
        !bodies[1]["context"]["commands"][0]
            .as_str()
            .unwrap()
            .contains(&secret)
    );
}

#[test]
fn bad_input_rejected_before_post() {
    let server = fake();
    let env = Env::new("hook-bad", &server.url);
    let run = |input: &str| {
        let config = env.config();
        let mut command = command(&env.home);
        command
            .current_dir(&env.repo)
            .args(["hook", "UserPromptSubmit", "--config"])
            .arg(&config);
        let output = with_stdin(&mut command, input);
        assert_eq!(output.status.code(), Some(0));
        output
    };
    let output = run("not json at all SECRETWORD");
    assert!(stderr(&output).contains("one JSON object") && !stderr(&output).contains("SECRETWORD"));
    let output = run(&json!({"cwd": env.repo, "prompt": "no session"}).to_string());
    assert!(
        stderr(&output).contains("session_id"),
        "{}",
        stderr(&output)
    );
    let output = run("[1, 2]");
    assert!(stderr(&output).contains("one JSON object"));
    // A token with whitespace, and a config without one, never reach the network.
    std::fs::write(
        env.config(),
        format!(r#"{{"url":"{}","token":"cdt bad token"}}"#, server.url),
    )
    .unwrap();
    let output = run(&env.payload("s1", json!({"prompt": "hi"})).to_string());
    assert!(
        stderr(&output).contains("one header token") && !stderr(&output).contains("bad token"),
        "{}",
        stderr(&output)
    );
    std::fs::write(env.config(), format!(r#"{{"url":"{}"}}"#, server.url)).unwrap();
    let output = run(&env.payload("s1", json!({"prompt": "hi"})).to_string());
    assert!(
        stderr(&output).contains("cairn setup"),
        "{}",
        stderr(&output)
    );
    assert!(server.requests().is_empty() && !env.outbox().exists());
}

#[test]
fn transport_rules() {
    // A base path prefixes every route.
    let server = fake();
    let env = Env::new("hook-base", &format!("{}/agent", server.url));
    env.prompt("s1", "hello");
    assert_eq!(server.requests()[0].path, "/agent/cairn-memory/observe");

    // A redirect is never followed, and the other origin sees nothing.
    let other = fake();
    let location = format!("{}/stolen", other.url);
    let redirecting = fake_with(move |_, _| (302, location.clone()));
    let env = Env::new("hook-redirect", &redirecting.url);
    env.prompt("s1", "hello");
    assert!(other.requests().is_empty());

    // Plain http off loopback is refused before any connection.
    let env = Env::new("hook-http", &server.url);
    std::fs::write(
        env.config(),
        format!(r#"{{"url":"http://192.0.2.1:9","token":"{TOKEN}"}}"#),
    )
    .unwrap();
    let started = Instant::now();
    let output = env.prompt("s1", "hello");
    assert!(
        stderr(&output).contains("plain http is allowed only for loopback"),
        "{}",
        stderr(&output)
    );
    assert!(started.elapsed() < Duration::from_secs(1) && !env.outbox().exists());
}

// ---- outbox -----------------------------------------------------------------------------

#[test]
fn connection_failure_queues_once_same_id() {
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    let env = Env::new("hook-down", &url);
    let output = env.prompt("s1", "hello");
    assert!(
        stderr(&output).contains("queued for replay"),
        "{}",
        stderr(&output)
    );
    assert!(!stderr(&output).contains(TOKEN));
    env.prompt("s1", "hello");
    let ids = queued_ids(&env.outbox());
    assert_eq!(ids.len(), 1, "the repeat is not queued twice");
    assert!(ids[0].starts_with("hk_"));
    let file = std::fs::read_to_string(env.outbox()).unwrap();
    assert!(!file.contains(TOKEN));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(env.outbox())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn a_server_5xx_queues_and_the_next_success_replays_it_with_the_same_id() {
    let healthy = Arc::new(Mutex::new(false));
    let flag = healthy.clone();
    let server = fake_with(move |_, request| {
        if *flag.lock().unwrap() {
            default_reply(request)
        } else {
            (503, json!({"error": "down"}).to_string())
        }
    });
    let env = Env::new("hook-replay-one", &server.url);
    env.prompt("s1", "first");
    let queued = queued_ids(&env.outbox());
    assert_eq!(queued.len(), 1);
    *healthy.lock().unwrap() = true;
    env.prompt("s1", "second");
    let sent: Vec<String> = server
        .observed()
        .iter()
        .map(|b| b["event_id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(sent.len(), 3);
    assert_eq!(
        sent[2], queued[0],
        "the queued body is replayed, not rebuilt"
    );
    assert_ne!(sent[1], queued[0]);
    assert!(queued_ids(&env.outbox()).is_empty());
}

#[test]
fn permanent_responses_not_queued() {
    for status in [400u16, 403, 404, 409] {
        let server = fake_with(move |_, _| (status, json!({"error": "no"}).to_string()));
        let env = Env::new(&format!("hook-perm-{status}"), &server.url);
        let output = env.prompt("s1", "hello");
        assert!(
            stderr(&output).contains(&format!("HTTP {status}")),
            "{}",
            stderr(&output)
        );
        assert!(!env.outbox().exists(), "{status} is dropped");
    }
    for status in [429u16, 500, 502] {
        let server = fake_with(move |_, _| (status, json!({"error": "later"}).to_string()));
        let env = Env::new(&format!("hook-retry-{status}"), &server.url);
        env.prompt("s1", "hello");
        assert_eq!(queued_ids(&env.outbox()).len(), 1, "{status} is queued");
    }
}

#[test]
fn outbox_binding_isolated() {
    let healthy = Arc::new(Mutex::new(false));
    let flag = healthy.clone();
    let server = fake_with(move |_, request| {
        if *flag.lock().unwrap() {
            default_reply(request)
        } else {
            (503, "{}".to_owned())
        }
    });
    let env = Env::new("hook-binding", &server.url);
    // A queue captured for another server sits at the default path.
    write_outbox(
        &env.outbox(),
        "https://elsewhere.example.test",
        TOKEN,
        &["hk_foreign".to_owned()],
    );
    let before = std::fs::read(env.outbox()).unwrap();
    env.prompt("s1", "mine");
    assert_eq!(
        std::fs::read(env.outbox()).unwrap(),
        before,
        "the foreign queue is untouched"
    );
    let own = env
        .home
        .join("cairn")
        .join(format!("outbox.{}.json", device_of(TOKEN)));
    assert_eq!(queued_ids(&own).len(), 1);
    assert!(!std::fs::read_to_string(&own).unwrap().contains(TOKEN));
    *healthy.lock().unwrap() = true;
    env.prompt("s1", "again");
    assert!(
        server
            .observed()
            .iter()
            .all(|b| b["event_id"] != "hk_foreign"),
        "never sent to another origin"
    );
}

#[test]
fn outbox_cap_and_disk_errors_keep_bytes() {
    let server = fake_with(|_, _| (503, "{}".to_owned()));
    let env = Env::new("hook-cap", &server.url);
    let ids: Vec<String> = (0..256).map(|i| format!("hk_old{i:03}")).collect();
    write_outbox(&env.outbox(), &server.url, TOKEN, &ids);
    let output = env.prompt("s1", "newest");
    let now = queued_ids(&env.outbox());
    assert_eq!(now.len(), 256);
    assert_eq!(now[0], "hk_old001", "the oldest was dropped");
    assert!(stderr(&output).contains("dropped 1"), "{}", stderr(&output));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let before = std::fs::read(env.outbox()).unwrap();
        let locked = env.home.join("cairn");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
        let output = env.prompt("s1", "cannot be written");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(!stderr(&output).is_empty());
        assert_eq!(std::fs::read(env.outbox()).unwrap(), before);
    }
}

#[test]
fn outbox_lock_never_blocks() {
    let server = fake();
    let env = Env::new("hook-lock", &server.url);
    write_outbox(
        &env.outbox(),
        &server.url,
        TOKEN,
        &["hk_waiting".to_owned()],
    );
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(env.home.join("cairn/outbox.json.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let started = Instant::now();
    env.prompt("s1", "delivered anyway");
    assert!(started.elapsed() < Duration::from_secs(1));
    let sent = server.observed();
    assert_eq!(
        sent.len(),
        1,
        "the capture is delivered, nothing is replayed"
    );
    assert_eq!(queued_ids(&env.outbox()), ["hk_waiting"]);

    // Busy and offline: the capture is not retained, and the hook does not wait.
    let down = fake_with(|_, _| (503, "{}".to_owned()));
    let env = Env::new("hook-lock-down", &down.url);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(env.home.join("cairn/outbox.json.lock"));
    let lock = lock.unwrap_or_else(|_| {
        std::fs::create_dir_all(env.home.join("cairn")).unwrap();
        std::fs::File::create(env.home.join("cairn/outbox.json.lock")).unwrap()
    });
    lock.try_lock().unwrap();
    let started = Instant::now();
    let output = env.prompt("s1", "lost");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(stderr(&output).contains("busy"), "{}", stderr(&output));
    assert!(!env.outbox().exists());
}

#[test]
fn concurrent_hooks_keep_every_id() {
    let server = fake();
    let env = Arc::new(Env::new("hook-concurrent", &server.url));
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let env = env.clone();
            thread::spawn(move || env.prompt("s1", &format!("prompt number {i}")))
        })
        .collect();
    for handle in handles {
        assert!(handle.join().unwrap().status.success());
    }
    let mut ids: Vec<String> = server
        .observed()
        .iter()
        .map(|b| b["event_id"].as_str().unwrap().to_owned())
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 8);
    // Each one also reached the repeat check: a second round sends nothing.
    for i in 0..8 {
        env.prompt("s1", &format!("prompt number {i}"));
    }
    assert_eq!(server.observed().len(), 8);
}

#[test]
fn replay_order_budget_and_401() {
    let server = fake();
    let env = Env::new("hook-replay", &server.url);
    let ids: Vec<String> = (0..25).map(|i| format!("hk_q{i:02}")).collect();
    write_outbox(&env.outbox(), &server.url, TOKEN, &ids);
    env.prompt("s1", "trigger");
    let sent: Vec<String> = server
        .observed()
        .iter()
        .map(|b| b["event_id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(&sent[1..], &ids[..20], "oldest twenty first, in order");
    assert_eq!(queued_ids(&env.outbox()), &ids[20..]);
    env.prompt("s1", "trigger two");
    assert!(queued_ids(&env.outbox()).is_empty(), "then the rest");

    // A 401 while replaying stops and keeps everything.
    let rejecting = fake_with(|index, request| {
        if index == 0 {
            default_reply(request)
        } else {
            (401, "{}".to_owned())
        }
    });
    let env = Env::new("hook-replay-401", &rejecting.url);
    write_outbox(&env.outbox(), &rejecting.url, TOKEN, &ids[..3]);
    env.prompt("s1", "trigger");
    assert_eq!(
        rejecting.observed().len(),
        2,
        "one replay attempt, then stop"
    );
    assert_eq!(queued_ids(&env.outbox()), &ids[..3]);

    // A stalled replay is bounded by the two second budget.
    let slow = fake_with(|index, request| {
        if index > 0 {
            thread::sleep(Duration::from_millis(1500));
        }
        default_reply(request)
    });
    let env = Env::new("hook-replay-slow", &slow.url);
    write_outbox(&env.outbox(), &slow.url, TOKEN, &ids[..10]);
    let started = Instant::now();
    env.prompt("s1", "trigger");
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    assert!(slow.observed().len() <= 3);
}

// ---- real server ------------------------------------------------------------------------

#[test]
fn a_real_server_accepts_every_capture_kind_and_serves_topic_notes() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "hook-real");
    let dir = checkout("hook-real-repo");
    let saved = team.owner.json_in(&dir, &["save", r#"{"kind":"gotcha","title":"Queue lock order","summary":"Take the queue lock before the row lock.","reason":"Deadlock otherwise.","fields":{"symptom":"hang","cause":"lock order","fix":"queue lock first"},"context":{"files":["src/queue.rs"]}}"#]);
    assert_eq!(saved["status"], "active");
    let config = team.owner.home.join("cairn").join("config.json");
    let run = |event: &str, payload: Value| {
        let mut command = command(&team.owner.home);
        command
            .current_dir(&dir)
            .args(["hook", event, "--agent", "claude", "--config"])
            .arg(&config);
        let output = with_stdin(&mut command, &payload.to_string());
        assert_eq!(stderr(&output), "", "{event}: {}", stderr(&output));
        output
    };
    let base = |extra: Value| {
        let mut payload = json!({"session_id": "real-1", "cwd": dir});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        payload
    };
    run("SessionStart", base(json!({"source": "startup"})));
    run("UserPromptSubmit", base(json!({"prompt": "fix the queue"})));
    let edit = run(
        "PostToolUse",
        base(edit_call("src/queue.rs", "fn lock() {}", "t1")),
    );
    let note = injected(&edit).expect("the file note is injected");
    assert!(
        note.contains("Queue lock order: Take the queue lock before the row lock."),
        "{note}"
    );
    assert!(
        injected(&run(
            "PostToolUse",
            base(edit_call("src/queue.rs", "fn lock2() {}", "t2"))
        ))
        .is_none()
    );
    run(
        "PostToolUse",
        base(
            json!({"tool_name": "Bash", "tool_input": {"command": "cargo test"}, "tool_response": {"exit_code": 1, "stderr": "boom"}}),
        ),
    );
    run("Stop", base(json!({})));
    run("SessionEnd", base(json!({})));
}
