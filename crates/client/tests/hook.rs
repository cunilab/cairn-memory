//! `cairn hook <Event>`: the real binary against a scripted fake server (transport, queueing,
//! injection, state) and against a real server on a `TestDb` schema (typed `/observe` bodies,
//! topic notes). No test needs Python.

mod common;

use cairn_client::Config;
use cairn_core::instructions::DEFAULT_SESSION_INSTRUCTION;
use common::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

const TOKEN: &str = "cdt_testtoken1234567890";
const NO_PROJECT_HINT: &str =
    "Cairn: no project here (personal memory). Run cairn init to share with a project.";

#[test]
fn claude_pre_tool_use_denies_only_writes_inside_native_memory() {
    let home = temp_dir("claude-pre-tool-use");
    let config = home.join("custom-claude");
    let project = config.join("projects/encoded-project");
    let memory = project.join("memory");
    std::fs::create_dir_all(&memory).unwrap();
    let invoke = |host: &str, name: &str, key: &str, path: &Path, custom: bool| {
        let mut cmd = command(&home);
        if custom {
            cmd.env("CLAUDE_CONFIG_DIR", &config);
        }
        let output = with_stdin(
            cmd.args(["hook", "PreToolUse", "--agent", host]),
            &json!({
                "cwd": project, "hook_event_name": "PreToolUse",
                "tool_name": name, "tool_input": {key: path}
            })
            .to_string(),
        );
        assert!(
            output.status.success() && output.stderr.is_empty(),
            "{output:?}"
        );
        output.stdout
    };
    let denied = json!({"hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "deny",
        "permissionDecisionReason": "Claude memory files are replaced by Cairn; save this with memory_save instead."
    }});
    for (name, key) in [
        ("Write", "file_path"),
        ("Edit", "file_path"),
        ("MultiEdit", "file_path"),
        ("NotebookEdit", "notebook_path"),
    ] {
        assert_eq!(
            serde_json::from_slice::<Value>(&invoke(
                "claude",
                name,
                key,
                &memory.join("note.md"),
                true
            ))
            .unwrap(),
            denied
        );
    }
    for path in [
        Path::new("memory/subdir/note.md"),
        Path::new("memory/../memory/note.md"),
    ] {
        assert!(!invoke("claude", "Write", "file_path", path, true).is_empty());
    }
    assert!(
        !invoke(
            "claude",
            "Write",
            "file_path",
            Path::new("~/.claude/projects/default/memory/note.md"),
            false
        )
        .is_empty()
    );
    #[cfg(unix)]
    {
        let alias = project.join("alias");
        std::os::unix::fs::symlink(&memory, &alias).unwrap();
        assert!(!invoke("claude", "Write", "file_path", &alias.join("new.md"), true).is_empty());
    }
    for (host, name, path) in [
        ("claude", "Read", "memory/note.md"),
        ("claude", "Bash", "memory/note.md"),
        ("claude", "Write", "memory-other/note.md"),
        ("claude", "Write", "memory/../src/note.md"),
        ("claude", "Write", "../../other/projects/id/memory/note.md"),
        ("codex", "Write", "memory/note.md"),
    ] {
        assert!(invoke(host, name, "file_path", Path::new(path), true).is_empty());
    }
    std::fs::remove_dir_all(home).unwrap();
}

fn brief_and_hint() -> String {
    format!("BRIEF\n{DEFAULT_SESSION_INSTRUCTION}\n{NO_PROJECT_HINT}")
}

#[derive(Clone)]
struct Req {
    path: String,
    auth: String,
    user_agent: String,
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
        user_agent: head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("user-agent")
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

#[test]
fn grok_hooks_capture_native_payloads_and_deliver_one_deferred_brief() {
    let fake = fake();
    let env = Env::new("grok-native-hook", &fake.url);
    let setup = command(&env.home).args(["setup", "grok"]).output().unwrap();
    assert!(setup.status.success(), "{setup:?}");
    let invoke = |event, extra: Value| {
        let mut payload = json!({"sessionId": "grok-session", "cwd": env.repo});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let out = with_stdin(
            command(&env.home).args(["hook", event, "--agent", "grok"]),
            &payload.to_string(),
        );
        assert!(out.status.success() && out.stderr.is_empty(), "{out:?}");
        out
    };
    assert!(invoke("SessionStart", json!({})).stdout.is_empty());
    assert!(fake.to("brief").is_empty());
    let imported = with_stdin(
        command(&env.home)
            .env("GROK_HOOK_EVENT", "user_prompt_submit")
            .args([
                "hook",
                "UserPromptSubmit",
                "--agent",
                "claude",
                "--cairn-managed",
            ]),
        &json!({"sessionId": "grok-session", "cwd": env.repo, "prompt": "Run the tests"})
            .to_string(),
    );
    assert!(imported.status.success(), "{imported:?}");
    assert!(fake.observed().is_empty());
    invoke("UserPromptSubmit", json!({"prompt": "Run the tests"}));
    let tool = json!({
        "toolName": "run_terminal_command",
        "toolUseId": "grok-tool-1",
        "toolInput": {"command": "cargo test"},
        "toolResult": {"exit_code": 101, "stderr": "first error\nand other output"},
    });
    assert_eq!(
        injected(&invoke("PostToolUse", tool.clone())).as_deref(),
        Some(brief_and_hint().as_str())
    );
    assert!(invoke("PostToolUse", tool).stdout.is_empty());
    let observed = fake.observed();
    assert_eq!(observed.len(), 2);
    assert_eq!(observed[0]["content"], "Run the tests");
    assert_eq!(observed[1]["content"], "cargo test\nexit 101: first error");
    assert_eq!(observed[1]["session_id"], "grok-session");
    assert_eq!(fake.to("brief").len(), 1);
    assert!(
        fake.to("observe")
            .iter()
            .all(|req| req.user_agent.contains("(grok; hook)"))
    );
    invoke("PreCompact", json!({}));
    assert_eq!(fake.to("checkpoint").len(), 1);
    invoke("SessionEnd", json!({}));
    assert!(!fake.to("end").is_empty());
}

#[test]
fn antigravity_hooks_capture_native_payloads_and_checkpoint_on_stop() {
    let fake = fake();
    let env = Env::new("antigravity-native-hook", &fake.url);
    let invoke = |event, extra: Value| {
        let mut payload =
            json!({"conversationId": "agy-conversation", "workspacePaths": [env.repo]});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let out = with_stdin(
            command(&env.home).current_dir(&env.home).args([
                "hook",
                event,
                "--agent",
                "antigravity",
            ]),
            &payload.to_string(),
        );
        assert!(out.status.success() && out.stderr.is_empty(), "{out:?}");
        serde_json::from_slice::<Value>(&out.stdout).unwrap_or(Value::Null)
    };
    let start = invoke("SessionStart", json!({}));
    assert_eq!(
        start["injectSteps"][0]["systemMessage"]["systemMessage"],
        brief_and_hint()
    );
    for invocation in 0..2 {
        invoke(
            "PreInvocation",
            json!({"invocationNum": invocation, "lastUserInput": "Edit the file"}),
        );
    }
    let edit = json!({
        "toolCall": {"name": "multi_replace_file_content", "args": {
            "TargetFile": env.repo.join("src.rs"),
            "ReplacementChunks": [{"ReplacementContent": "first"}, {"ReplacementContent": "second"}]
        }}
    });
    assert_eq!(invoke("PostToolUse", edit), json!({}));
    invoke(
        "PostToolUse",
        json!({
            "toolCall": {"name": "run_command", "args": {"CommandLine": "cargo test"}},
            "error": "exit status 1",
        }),
    );
    let out = invoke(
        "Stop",
        json!({"fullyIdle": true, "terminationReason": "model_stop"}),
    );
    assert_eq!(out, json!({"decision": "stop"}));
    let observed = fake.observed();
    assert_eq!(observed.len(), 4, "{observed:?}");
    assert_eq!(observed[0]["content"], "Edit the file");
    assert_eq!(observed[1]["content"], "MultiEdit src.rs\nfirst\nsecond");
    assert_eq!(observed[2]["content"], "cargo test\nfailed: exit status 1");
    assert_eq!(observed[2]["session_id"], "agy-conversation");
    assert_eq!(observed[3]["content"], "changed files\nsrc.rs");
    assert_eq!(fake.to("checkpoint").len(), 1);
    assert!(fake.to("topic").is_empty());
    assert!(
        fake.to("observe")
            .iter()
            .all(|req| req.user_agent.contains("(antigravity; hook)"))
    );
}

#[test]
fn hermes_hooks_capture_native_prompts_edits_commands_and_lookups() {
    let fake = fake();
    let env = Env::new("hermes-native-hook", &fake.url);
    let invoke = |event, extra: Value| {
        let mut payload = json!({"session_id": "hermes-session", "cwd": env.repo});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let out = with_stdin(
            command(&env.home)
                .env("CAIRN_CAPTURE_ASSISTANT", "1")
                .args(["hook", event, "--agent", "hermes"]),
            &payload.to_string(),
        );
        assert!(out.status.success() && out.stderr.is_empty(), "{out:?}");
        out
    };
    assert_eq!(
        injected(&invoke("SessionStart", json!({}))).as_deref(),
        Some(brief_and_hint().as_str())
    );
    invoke(
        "UserPromptSubmit",
        json!({"user_message": "Edit and test the widget"}),
    );
    let terminal = json!({
        "tool_name": "terminal", "tool_call_id": "cmd-1", "args": {"command": "cargo test"},
        "result": r#"{"exit_code":101,"output":"","error":"first error"}"#,
        "status": "error", "error_message": "first error",
    });
    assert!(invoke("PreToolUse", terminal.clone()).stdout.is_empty());
    assert_eq!(fake.observed().len(), 1);
    invoke("PostToolUse", terminal);
    for (status, path) in [("blocked", "blocked.rs"), ("ok", "widget.rs")] {
        invoke(
            "PostToolUse",
            json!({
                "tool_name": "write_file", "tool_call_id": path,
                "args": {"path": path, "content": "written contents"}, "result": "{}", "status": status,
            }),
        );
    }
    invoke(
        "PostToolUse",
        json!({
            "tool_name": "mcp__cairn__memory_search", "tool_call_id": "search-1",
            "args": {"query": "widgets"}, "result": json!({"result": r#"{"results":[]}"#}).to_string(),
            "status": "ok",
        }),
    );
    assert!(
        invoke("Stop", json!({"assistant_response": "Verified the patch"}))
            .stdout
            .is_empty()
    );
    let observed = fake.observed();
    assert_eq!(
        observed
            .iter()
            .map(|row| row["content"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "Edit and test the widget",
            "cargo test\nexit 101: first error",
            "Write widget.rs\nwritten contents",
            "Verified the patch",
            "changed files\nwidget.rs",
        ]
    );
    assert!(
        fake.to("observe")
            .iter()
            .all(|req| req.user_agent.contains("(hermes; hook)"))
    );
    assert_eq!(fake.to("brief").len(), 1);
    assert_eq!(fake.to("lookup").len(), 1);
    assert_eq!(fake.to("checkpoint").len(), 1);
    assert!(fake.to("topic").is_empty());
    invoke(
        "PostToolUse",
        json!({
            "tool_name": "mcp__cairn__memory_save", "tool_call_id": "save-1",
            "args": {}, "result": json!({"result": r#"{"id":"mem_test","deduplicated":false}"#}).to_string(),
            "status": "ok",
        }),
    );
    invoke("PreCompact", json!({}));
    invoke("SessionEnd", json!({}));
    assert_eq!(fake.to("end").len(), 1);
    assert_eq!(fake.to("end")[0].body["nudge_saved"], true);
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

/// Polls `done` for at most ten seconds; the tests below use it to sync on what a server saw,
/// never on elapsed time.
fn wait_until(done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "condition not reached in 10 s");
        thread::sleep(Duration::from_millis(10));
    }
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
    assert_eq!(injected(&start).as_deref(), Some(brief_and_hint().as_str()));
    let value: Value = serde_json::from_slice(&start.stdout).unwrap();
    assert_eq!(value["hookSpecificOutput"]["hookEventName"], "SessionStart");
    let brief = &server.to("brief")[0];
    assert_eq!(brief.auth, format!("Bearer {TOKEN}"));
    // The console shows each device's client version and agent from this header.
    assert_eq!(
        brief.user_agent,
        format!("cairn/{} (claude; hook)", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(brief.body["session_id"], "s1");
    assert_eq!(brief.body["context"]["repository"], "local:repo");
    assert_eq!(brief.body["recent_files"], json!([]));

    // No work: SessionEnd closes nothing on the server and removes the state.
    env.run("SessionEnd", &env.payload("s1", json!({})));
    assert!(server.to("end").is_empty());

    env.edit("s2", "src/a.rs", "fn a() {}");
    assert!(injected(&env.run("SessionEnd", &env.payload("s2", json!({})))).is_none());
    assert_eq!(server.to("end").len(), 1);
    assert_eq!(server.to("end")[0].body["session_id"], "s2");
    // The lock files stay for `sweep`; the state itself is gone.
    let left = std::fs::read_dir(env.state_dir())
        .unwrap()
        .filter(|entry| {
            entry
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|e| e == "json")
        })
        .count();
    assert_eq!(left, 0, "state files are deleted at SessionEnd");
}

#[test]
fn session_start_reports_repo_state() {
    let check = json!({"project_id": "prj_demo", "branch": "main",
                       "files": ["keep.txt", "gone.txt", "new.txt"], "symbols": ["keep_symbol", "gone_symbol"]});
    let reply = check.clone();
    let server = fake_with(move |_, request| {
        if request.path.ends_with("/brief") {
            return (200, json!({"brief": "BRIEF", "check": reply}).to_string());
        }
        default_reply(request)
    });
    let env = Env::new("hook-repo-state", &server.url);
    let repo = &env.repo;
    std::fs::create_dir_all(repo.join(".cairn")).unwrap();
    std::fs::write(
        repo.join(".cairn/project.json"),
        r#"{"project_id":"prj_demo"}"#,
    )
    .unwrap();
    std::fs::write(repo.join("keep.txt"), "fn keep_symbol() {}\n").unwrap();
    git(repo, &["add", "keep.txt"]);
    git(repo, &["commit", "-q", "-m", "keep"]);
    // an untracked, not ignored file still counts as present; a merged branch is reported, an unmerged one is not
    std::fs::write(repo.join("new.txt"), "x").unwrap();
    git(repo, &["branch", "merged-feature"]);
    git(repo, &["checkout", "-q", "-b", "unmerged"]);
    git(repo, &["commit", "-q", "--allow-empty", "-m", "ahead"]);
    git(repo, &["checkout", "-q", "main"]);
    let start = env.run(
        "SessionStart",
        &env.payload("s1", json!({"source": "startup"})),
    );
    // A project file is there, so the brief carries the instruction line.
    assert!(
        injected(&start)
            .unwrap()
            .starts_with(&format!("BRIEF\n{DEFAULT_SESSION_INSTRUCTION}"))
    );
    let posts = server.to("repo-state");
    assert_eq!(
        posts.len(),
        1,
        "{:?}",
        server
            .requests()
            .iter()
            .map(|r| &r.path)
            .collect::<Vec<_>>()
    );
    let body = &posts[0].body;
    assert_eq!(posts[0].auth, format!("Bearer {TOKEN}"));
    assert_eq!(body["project_id"], "prj_demo");
    assert_eq!(body["branch"], "main");
    assert_eq!(body["commit"].as_str().map(str::len), Some(40));
    assert_eq!(body["files_missing"], json!(["gone.txt"]));
    assert_eq!(body["files_present"], json!(["keep.txt", "new.txt"]));
    assert_eq!(body["symbols_present"], json!(["keep_symbol"]));
    assert_eq!(body["symbols_missing"], json!(["gone_symbol"]));
    assert_eq!(body["merged_branches"], json!(["merged-feature"]));
    // no check in the reply: no report
    let quiet = fake();
    let env = Env::new("hook-repo-state-none", &quiet.url);
    env.run("SessionStart", &env.payload("s1", json!({})));
    assert!(quiet.to("repo-state").is_empty());
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
    assert_eq!(
        injected(&start).as_deref(),
        Some(DEFAULT_SESSION_INSTRUCTION)
    );
    assert!(prompt.stdout.is_empty());
    for output in [&start, &prompt] {
        let text = stderr(output);
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
    // Read-only calls are never captured; a Read only asks for the file's topic notes.
    let paths: Vec<String> = server.requests().into_iter().map(|r| r.path).collect();
    assert!(
        paths.len() == 1 && paths[0].ends_with("/topic"),
        "{paths:?}"
    );
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
    assert_eq!(
        injected(&compact).as_deref(),
        Some(brief_and_hint().as_str())
    );
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
    assert_eq!(
        injected(&again).as_deref(),
        Some(format!("BRIEF\n{DEFAULT_SESSION_INSTRUCTION}").as_str())
    );
}

fn mcp_instructions(home: &Path, checkout: &Path, session_id: Option<&str>) -> String {
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "_meta": {"sessionId": session_id}}
    });
    let output = with_stdin(
        command(home).current_dir(checkout).arg("mcp"),
        &request.to_string(),
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    reply["result"]["instructions"].as_str().unwrap().to_owned()
}

#[test]
fn mcp_initialize_falls_back_to_static_instructions_on_brief_timeout() {
    let server = fake_with(|_, request| {
        if request.path.ends_with("/brief") {
            thread::sleep(Duration::from_secs(4));
        }
        default_reply(request)
    });
    let env = Env::new("mcp-brief-timeout", &server.url);
    let start = Instant::now();
    let instructions = mcp_instructions(&env.home, &env.repo, Some("timeout"));
    assert_eq!(instructions, cairn_client::tools::instructions());
    assert!(start.elapsed() < Duration::from_secs(3));
    let requests = server.to("brief");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["session_id"], "timeout");
    assert_eq!(requests[0].body["context"]["repository"], "local:repo");
}

#[test]
fn mcp_and_hermes_session_start_deduplicate_the_same_brief() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "mcp-hermes");
    let dir = checkout("mcp-hermes");
    let saved = team.owner.json_in(&dir, &["save", r#"{"kind":"preference","title":"Keep the queue visible","summary":"Show the queue when reviewing worker health.","reason":"Operators spot blocked work in that view."}"#]);
    let id = saved["id"].as_str().unwrap();
    let initialize = |session| mcp_instructions(&team.owner.home, &dir, session);
    let hermes_start = |session| {
        let output = with_stdin(
            command(&team.owner.home).current_dir(&dir).args([
                "hook",
                "SessionStart",
                "--agent",
                "hermes",
            ]),
            &json!({"session_id": session, "cwd": dir}).to_string(),
        );
        assert!(output.status.success(), "{}", stderr(&output));
        injected(&output).unwrap()
    };
    let text = initialize(Some("mcp-first"));
    let brief = text
        .strip_prefix(cairn_client::tools::instructions())
        .unwrap()
        .trim_start();
    assert!(
        brief.contains(id) && brief.contains(r#""why":"personal preference""#),
        "{text}"
    );
    assert!(brief.len() <= 1200);
    assert!(!hermes_start("mcp-first").contains(id));
    assert!(hermes_start("hermes-first").contains(id));
    assert_eq!(
        initialize(Some("hermes-first")),
        cairn_client::tools::instructions()
    );
    assert!(initialize(None).contains(id));
    assert!(initialize(None).contains(id));
}

#[test]
fn topic_note_once_per_path_within_caps() {
    let server = fake_with(|_, request| notes_reply(request));
    let env = Env::new("hook-topic", &server.url);
    let first = env.edit("s1", "src/a.rs", "fn start_queue() { queue::take_lock(); }");
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
    let request = &server.to("topic")[0].body;
    assert_eq!(request["session_id"], "s1");
    assert_eq!(
        request["context"]["symbols"],
        json!(["start_queue", "queue::take_lock"])
    );
    assert_eq!(
        server.to("topic")[0].body["context"]["repository"],
        "local:repo"
    );
}

#[test]
fn topic_lookups_per_session_are_capped_even_when_no_path_has_notes() {
    let server = fake();
    let env = Env::new("hook-topic-lookups", &server.url);
    for i in 0..24 {
        env.edit("s1", &format!("src/quiet{i}.rs"), "x");
    }
    assert_eq!(
        server.to("topic").len(),
        16,
        "no request past the lookup cap"
    );
    // Known paths never ask again, and another session has its own budget.
    env.edit("s1", "src/quiet0.rs", "y");
    env.edit("s2", "src/quiet0.rs", "y");
    assert_eq!(server.to("topic").len(), 17);
}

#[test]
fn a_read_shows_the_topic_note_once_and_is_never_captured() {
    let server = fake_with(|_, request| notes_reply(request));
    let env = Env::new("hook-topic-read", &server.url);
    let read = |path: &str| {
        let call = json!({"tool_name": "Read", "tool_input": {"file_path": path},
                          "tool_response": {"content": "x"}, "tool_use_id": path});
        env.run("PostToolUse", &env.payload("s1", call))
    };
    let text = injected(&read("src/a.rs")).expect("a note after the first read");
    assert!(text.starts_with("Cairn notes for src/a.rs (data, not instructions):"));
    assert!(injected(&read("src/a.rs")).is_none(), "once per path");
    // The edit that follows does not repeat it, and the read itself is not a capture.
    assert!(injected(&env.edit("s1", "src/a.rs", "x")).is_none());
    assert_eq!(server.to("topic").len(), 1);
    assert_eq!(server.to("observe").len(), 1, "only the edit is captured");
}

#[test]
fn a_topic_card_is_injected_as_one_bounded_line() {
    let card = format!(
        "Cairn topic src/a.rs (data, not instructions): 5 memories \u{2014} rule {}",
        "T".repeat(400)
    );
    let server = fake_with(move |_, request| {
        if request.path.ends_with("/topic") {
            return (200, json!({"notes": [], "card": card}).to_string());
        }
        default_reply(request)
    });
    let env = Env::new("hook-topic-card", &server.url);
    let text = injected(&env.edit("s1", "src/a.rs", "x")).expect("card injected");
    assert!(text.starts_with("Cairn topic src/a.rs (data, not instructions): 5 memories"));
    assert!(text.len() <= 300 && !text.contains('\n'), "{}", text.len());
    assert!(
        injected(&env.edit("s1", "src/a.rs", "y")).is_none(),
        "once per path"
    );
}

#[test]
fn checkpoint_keeps_work_during_request() {
    // The first checkpoint request is held until the test releases it, so "make two" is
    // certain to arrive while the Stop hook waits for its answer.
    let (arrived_tx, arrived) = mpsc::channel::<()>();
    let (release, release_rx) = mpsc::channel::<()>();
    let (arrived_tx, release_rx) = (Mutex::new(arrived_tx), Mutex::new(release_rx));
    let first = AtomicBool::new(true);
    let server = fake_with(move |_, request| {
        if request.path.ends_with("/checkpoint") && first.swap(false, Ordering::SeqCst) {
            arrived_tx.lock().unwrap().send(()).unwrap();
            let _ = release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(30));
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
    arrived
        .recv_timeout(Duration::from_secs(30))
        .expect("the checkpoint request arrived");
    env.bash("s1", "make two", json!({"exit_code": 0}));
    release.send(()).unwrap();
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
            keys.iter().all(|k| [
                "content",
                "context",
                "event_id",
                "kind",
                "ok",
                "outbox_depth",
                "session_id"
            ]
            .contains(k)),
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
    // Refused before any connection: nothing queued, and well inside the host's limit.
    assert!(started.elapsed() < Duration::from_secs(5) && !env.outbox().exists());
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
fn a_held_outbox_lock_is_survived_without_losing_or_inventing_events() {
    // Locked from outside for the whole hook (a stuck process): a delivered capture replays
    // nothing and leaves the queue as it was.
    let server = fake();
    let env = Env::new("hook-lock", &server.url);
    write_outbox(
        &env.outbox(),
        &server.url,
        TOKEN,
        &["hk_waiting".to_owned()],
    );
    let before = std::fs::read(env.outbox()).unwrap();
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(env.home.join("cairn/outbox.json.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    env.prompt("s1", "delivered anyway");
    assert_eq!(server.observed().len(), 1, "delivered, nothing replayed");
    assert_eq!(std::fs::read(env.outbox()).unwrap(), before);

    // Locked and offline: the capture cannot be retained and the hook says so.
    let down = fake_with(|_, _| (503, "{}".to_owned()));
    let env = Env::new("hook-lock-down", &down.url);
    std::fs::create_dir_all(env.home.join("cairn")).unwrap();
    let lock = std::fs::File::create(env.home.join("cairn/outbox.json.lock")).unwrap();
    lock.try_lock().unwrap();
    let output = env.prompt("s1", "cannot be kept");
    assert!(stderr(&output).contains("busy"), "{}", stderr(&output));
    assert!(!env.outbox().exists());
}

#[test]
fn a_failing_capture_is_queued_while_another_hook_replays() {
    // Ten old events are queued and the server answers them slowly. Session "a" is delivered, so
    // its hook replays them. Session "b" is refused while that replay is running and must still
    // be kept.
    let server = fake_with(|_, request| match request.body["session_id"].as_str() {
        Some("old") => {
            thread::sleep(Duration::from_millis(400));
            default_reply(request)
        }
        Some("b") => (503, "{}".to_owned()),
        _ => default_reply(request),
    });
    let env = Arc::new(Env::new("hook-replay-lock", &server.url));
    let ids: Vec<String> = (0..10).map(|i| format!("hk_old{i:02}")).collect();
    write_outbox(&env.outbox(), &server.url, TOKEN, &ids);
    let replaying = {
        let env = env.clone();
        thread::spawn(move || env.prompt("a", "delivered"))
    };
    wait_until(|| {
        server
            .requests()
            .iter()
            .any(|r| r.body["session_id"] == "old")
    });
    let refused = env.prompt("b", "refused while replaying");
    assert!(replaying.join().unwrap().status.success());
    assert!(!stderr(&refused).contains("busy"), "{}", stderr(&refused));
    let refused_id = server
        .observed()
        .into_iter()
        .find(|body| body["session_id"] == "b")
        .map(|body| body["event_id"].as_str().unwrap().to_owned())
        .unwrap();
    assert!(
        queued_ids(&env.outbox()).contains(&refused_id),
        "the capture refused during the replay was kept"
    );
}

#[test]
fn a_flapping_server_loses_no_capture_across_many_processes() {
    let delivered: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = delivered.clone();
    let server = fake_with(move |index, request| {
        if index % 3 == 0 {
            return (503, "{}".to_owned());
        }
        log.lock()
            .unwrap()
            .push(request.body["event_id"].as_str().unwrap_or("").to_owned());
        default_reply(request)
    });
    let env = Arc::new(Env::new("hook-flap", &server.url));
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let env = env.clone();
            thread::spawn(move || env.prompt("s1", &format!("flap number {i}")))
        })
        .collect();
    for handle in handles {
        let output = handle.join().unwrap();
        assert!(
            !stderr(&output).contains("not retained"),
            "{}",
            stderr(&output)
        );
    }
    let mut kept: Vec<String> = delivered.lock().unwrap().clone();
    kept.extend(
        std::fs::read(env.outbox()).map_or_else(|_| Vec::new(), |_| queued_ids(&env.outbox())),
    );
    kept.sort();
    kept.dedup();
    assert_eq!(kept.len(), 8, "every capture is delivered or still queued");
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
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(slow.observed().len() <= 3);
}

// ---- review findings -----------------------------------------------------------------------

#[test]
fn a_dead_server_costs_one_timeout_not_one_per_call() {
    // The server accepts and never answers within the 2 s request timeout.
    let server = fake_with(|_, request| {
        thread::sleep(Duration::from_secs(5));
        default_reply(request)
    });
    let env = Env::new("hook-slow", &server.url);
    let first = env.edit("s1", "src/a.rs", "x");
    assert!(stderr(&first).contains("queued"), "{}", stderr(&first));
    assert_eq!(
        server.requests().len(),
        1,
        "no topic lookup after the capture timed out"
    );
    // Marked down: the next hooks queue without asking, so none comes near the 2 s timeout.
    for i in 0..3 {
        let started = Instant::now();
        env.prompt("s1", &format!("later {i}"));
        assert!(
            started.elapsed() < Duration::from_millis(1900),
            "hook {i} waited on a server that is marked down"
        );
    }
    assert_eq!(
        server.requests().len(),
        1,
        "the dead server is not asked again"
    );
    assert_eq!(queued_ids(&env.outbox()).len(), 4);
}

#[test]
fn parallel_edits_of_one_file_inject_one_note() {
    // Release both edits from capture together so they compete to reserve the topic lookup.
    let gate = Arc::new((Mutex::new(0usize), Condvar::new()));
    let wait = gate.clone();
    let server = fake_with(move |_, request| {
        if request.path.ends_with("/observe") {
            let (count, arrived) = &*wait;
            let mut count = count.lock().unwrap();
            *count += 1;
            arrived.notify_all();
            let _ = arrived
                .wait_timeout_while(count, Duration::from_secs(5), |n| *n < 2)
                .unwrap();
        }
        notes_reply(request)
    });
    let env = Arc::new(Env::new("hook-topic-race", &server.url));
    let handles: Vec<_> = (0..2)
        .map(|i| {
            let env = env.clone();
            thread::spawn(move || env.edit("s1", "src/a.rs", &format!("version {i}")))
        })
        .collect();
    let notes = handles
        .into_iter()
        .map(|handle| injected(&handle.join().unwrap()))
        .filter(Option::is_some)
        .count();
    assert_eq!(notes, 1, "the note is injected once");
    assert_eq!(server.to("topic").len(), 1, "one hook reserves the lookup");
}

#[test]
fn a_rejected_capture_is_sent_again_once_the_cause_is_fixed() {
    let healthy = Arc::new(Mutex::new(false));
    let flag = healthy.clone();
    let server = fake_with(move |_, request| {
        if *flag.lock().unwrap() {
            default_reply(request)
        } else {
            (401, "{}".to_owned())
        }
    });
    let env = Env::new("hook-resend", &server.url);
    let first = env.prompt("s1", "the same prompt");
    assert!(stderr(&first).contains("401"), "{}", stderr(&first));
    *healthy.lock().unwrap() = true;
    env.prompt("s1", "the same prompt");
    assert_eq!(
        server.observed().len(),
        2,
        "a capture that never arrived is not remembered as sent"
    );
    env.prompt("s1", "the same prompt");
    assert_eq!(server.observed().len(), 2, "a delivered one is");
}

#[test]
fn apply_patch_headers_become_one_edit_per_file() {
    let server = fake_with(|_, request| notes_reply(request));
    let env = Env::new("hook-patch", &server.url);
    let patch = "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-old\n+new a\n*** Add File: src/b.rs\n+new b\n*** Delete File: old.rs\n*** End Patch";
    let output = env.run(
        "PostToolUse",
        &env.payload(
            "s1",
            json!({"tool_name": "apply_patch", "tool_input": {"command": patch}, "tool_response": {}}),
        ),
    );
    let edits: Vec<(String, Value)> = server
        .observed()
        .into_iter()
        .filter(|body| body["kind"] == "edit")
        .map(|body| {
            (
                body["content"].as_str().unwrap().to_owned(),
                body["context"]["files"].clone(),
            )
        })
        .collect();
    assert_eq!(edits.len(), 3, "{edits:?}");
    for (path, (content, files)) in ["src/a.rs", "src/b.rs", "old.rs"].iter().zip(&edits) {
        assert!(
            content.starts_with(&format!("apply_patch {path}\n")),
            "{content}"
        );
        assert_eq!(files, &json!([path]));
    }
    assert!(edits[0].0.contains("+new a") && !edits[0].0.contains("new b"));
    assert_eq!(server.to("topic").len(), 3, "a topic lookup per path");
    assert!(injected(&output).unwrap().contains("src/a.rs"));
}

#[cfg(unix)]
#[test]
fn an_edit_with_a_topic_note_runs_three_git_calls() {
    // top level, origin, branch: read once per hook run and shared by the capture, the topic
    // request and the ignore check.
    use std::os::unix::fs::PermissionsExt;
    let server = fake();
    let env = Env::new("hook-git-count", &server.url);
    let bin = env.home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = env.home.join("git-calls.log");
    let real = String::from_utf8(
        std::process::Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    std::fs::write(
        bin.join("git"),
        format!(
            "#!/bin/sh\necho call >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            real.trim()
        ),
    )
    .unwrap();
    std::fs::set_permissions(bin.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    env.run_with(
        "PostToolUse",
        &env.payload("s1", edit_call("src/a.rs", "fn a() {}", "t1")),
        &[],
        &[("PATH", &path)],
    );
    assert_eq!(server.to("observe").len() + server.to("topic").len(), 2);
    let calls = std::fs::read_to_string(&log).unwrap().lines().count();
    assert_eq!(calls, 3, "git ran {calls} times");
}

#[cfg(unix)]
#[test]
fn a_hanging_git_does_not_hold_the_hook() {
    use std::os::unix::fs::PermissionsExt;
    let server = fake();
    let env = Env::new("hook-git-hang", &server.url);
    let bin = env.home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("git"), "#!/bin/sh\nexec sleep 8\n").unwrap();
    std::fs::set_permissions(bin.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let started = Instant::now();
    let output = env.run_with(
        "SessionStart",
        &env.payload("s1", json!({"source": "startup"})),
        &[],
        &[("PATH", &path)],
    );
    assert_eq!(
        injected(&output).as_deref(),
        Some(brief_and_hint().as_str()),
        "{}",
        stderr(&output)
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "git calls are bounded: {:?}",
        started.elapsed()
    );
}

// ---- real server ------------------------------------------------------------------------

#[test]
fn mcp_search_finds_a_checkpoint_note_and_handoff_in_the_same_checkout() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "hook-recall");
    let dir = checkout("hook-recall");
    let init = team.owner.run_in(&dir, &["init", "E2E Hook Recall"]);
    assert!(init.status.success(), "{}", stderr(&init));

    let run = |event, session, extra: Value| {
        let mut payload = json!({"session_id": session, "cwd": dir});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let output = with_stdin(
            command(&team.owner.home)
                .current_dir(&dir)
                .args(["hook", event, "--agent", "claude"]),
            &payload.to_string(),
        );
        assert!(output.status.success(), "{event}: {}", stderr(&output));
        assert_eq!(stderr(&output), "", "{event}");
        output
    };
    run(
        "UserPromptSubmit",
        "recall",
        json!({"prompt": "Decision: bound queue concurrency\nReason: protect the worker pool\nTODO: verify exponential backoff"}),
    );
    run(
        "PostToolUse",
        "recall",
        edit_call("src/queue.rs", "fn run() {}", "edit"),
    );
    run("Stop", "recall", json!({}));
    assert_eq!(server.distill(&team.owner_id, "recall"), 1);
    run("SessionEnd", "recall", json!({}));
    server.distill(&team.owner_id, "recall");

    let brief = injected(&run("SessionStart", "next-session", json!({}))).unwrap();
    assert!(brief.contains("verify exponential backoff"), "{brief}");
    assert!(!brief.contains("bound queue concurrency"), "{brief}");

    let mut mcp = team.owner.mcp_in(&dir);
    let note = mcp.ok("memory_search", json!({"query": "bound queue concurrency"}));
    assert_eq!(note["results"][0]["kind"], "decision", "{note}");
    let handoff = mcp.ok(
        "memory_search",
        json!({"query": "verify exponential backoff"}),
    );
    assert_eq!(handoff["results"][0]["kind"], "handoff", "{handoff}");
}

#[test]
fn real_hook_switches_context_within_one_session_without_sharing_notes() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "hook-scopes");
    let personal = temp_dir("hook-scopes-personal");
    let a = checkout("hook-scopes-project-a");
    let b = checkout("hook-scopes-project-b");
    for (dir, remote) in [
        (&a, "https://example.test/a.git"),
        (&b, "https://example.test/b.git"),
    ] {
        assert!(
            std::process::Command::new("git")
                .current_dir(dir)
                .args(["remote", "add", "origin", remote])
                .status()
                .unwrap()
                .success()
        );
    }
    for dir in [&a, &b] {
        assert!(team.owner.run_in(dir, &["init"]).status.success());
    }
    let run = |dir: &std::path::Path, event, session, extra: Value| {
        let mut payload = json!({"session_id":session,"cwd":dir});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let output = with_stdin(
            command(&team.owner.home)
                .current_dir(dir)
                .args(["hook", event, "--agent", "claude"]),
            &payload.to_string(),
        );
        assert!(output.status.success(), "{event}: {}", stderr(&output));
    };
    for (session, first, last) in [
        ("personal-to-a", &personal, &a),
        ("a-to-b", &a, &b),
        ("b-to-personal", &b, &personal),
    ] {
        for (i, dir) in [first, last].into_iter().enumerate() {
            let extra = json!({"prompt":format!("Decision: keep {session} turn {i} isolated\nReason: capture scope follows checkout")});
            run(dir, "UserPromptSubmit", session, extra.clone());
            run(dir, "UserPromptSubmit", session, extra);
        }
        run(last, "SessionEnd", session, json!({}));
        assert_eq!(server.distill(&team.owner_id, session), 2);
    }
    let list = team
        .owner_web
        .get("/cairn-memory/memories?kind=decision")
        .unwrap();
    let decisions = list.as_array().unwrap();
    assert_eq!(decisions.len(), 6);
    assert!(
        decisions
            .iter()
            .all(|row| row["scope"] == "personal" && row["project_id"].is_null())
    );
}

#[test]
fn digest_uses_last_command_execution_even_when_its_failure_is_identical() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "hook-test-order");
    let dir = checkout("hook-test-order");
    let run = |event, extra: Value| {
        let mut payload = json!({"session_id":"test-order","cwd":dir});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let output = with_stdin(
            command(&team.owner.home)
                .current_dir(&dir)
                .args(["hook", event, "--agent", "claude"]),
            &payload.to_string(),
        );
        assert!(output.status.success(), "{event}: {}", stderr(&output));
    };
    run("PostToolUse", edit_call("src/a.rs", "fn foo() {}", "edit"));
    for (id, code) in [("first", 1), ("second", 0), ("third", 1), ("third", 1)] {
        run(
            "PostToolUse",
            json!({"tool_name":"Bash", "tool_input":{"command":"cargo test"},
             "tool_use_id":id, "tool_response":{"exit_code":code}}),
        );
    }
    run("SessionEnd", json!({}));
    server.distill(&team.owner_id, "test-order");
    let list = team
        .owner_web
        .get("/cairn-memory/memories?kind=note")
        .unwrap();
    let digest = &list[0]["summary"].as_str().unwrap();
    assert!(digest.contains("cargo test fail"), "{digest}");
}

#[test]
fn a_real_server_accepts_every_capture_kind_and_serves_topic_notes() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "hook-real");
    let dir = checkout("hook-real-repo");
    let saved = team.owner.json_in(&dir, &["save", r#"{"kind":"gotcha","title":"Queue lock order","summary":"Take the queue lock before the row lock.","reason":"Inverting the order can deadlock workers.","fields":{"symptom":"hang","cause":"lock order","fix":"queue lock first"},"context":{"symbols":["queue::take_lock"]}}"#]);
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
        base(edit_call(
            "src/queue.rs",
            "fn lock() { queue::take_lock(); }",
            "t1",
        )),
    );
    let note = injected(&edit).expect("the file note is injected");
    assert!(
        note.contains("Queue lock order: Take the queue lock before the row lock. (why: symbol queue::take_lock)"),
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

// ---- Agent nudges: the start line and the one-time Stop reminder ----

fn with_project(env: &Env) {
    std::fs::create_dir_all(env.repo.join(".cairn")).unwrap();
    std::fs::write(
        env.repo.join(".cairn/project.json"),
        r#"{"project_id":"prj_demo"}"#,
    )
    .unwrap();
}

/// The reason of a Stop reply that makes the agent continue; `None` for no output.
fn blocked(output: &Output) -> Option<String> {
    let text = String::from_utf8_lossy(&output.stdout);
    if text.trim().is_empty() {
        return None;
    }
    let value: Value = serde_json::from_str(&text).expect("stdout is one JSON object");
    assert_eq!(value["decision"], "block", "{text}");
    Some(value["reason"].as_str().unwrap().to_owned())
}

fn start(env: &Env, session: &str) -> Option<String> {
    injected(&env.run(
        "SessionStart",
        &env.payload(session, json!({"source": "startup"})),
    ))
}

fn stop(env: &Env, session: &str, extra: Value) -> Output {
    env.run("Stop", &env.payload(session, extra))
}

#[test]
fn start_appends_one_instruction_line_only_in_an_initialised_repo() {
    let server = fake();
    let env = Env::new("hook-nudge-start", &server.url);
    assert_eq!(
        start(&env, "s0").as_deref(),
        Some(brief_and_hint().as_str()),
        "no project file: the instruction precedes the hint"
    );
    with_project(&env);
    let text = start(&env, "s1").unwrap();
    let (brief, line) = text.split_once('\n').unwrap();
    assert_eq!(brief, "BRIEF");
    assert!(line == DEFAULT_SESSION_INSTRUCTION, "{line}");
    assert!(
        line.contains("memory_search") && line.contains("memory_save"),
        "{line}"
    );
    assert!(line.len() <= 280, "{}", line.len());
}

#[test]
fn start_gives_the_line_even_when_the_project_has_no_brief_yet() {
    let server = fake_with(|_, request| {
        if request.path.ends_with("/brief") {
            return (200, json!({"brief": ""}).to_string());
        }
        default_reply(request)
    });
    let env = Env::new("hook-nudge-empty", &server.url);
    with_project(&env);
    let text = start(&env, "s1").unwrap();
    assert!(text == DEFAULT_SESSION_INSTRUCTION, "{text}");
}

#[test]
fn work_without_a_save_blocks_the_first_stop_once() {
    let server = fake();
    let env = Env::new("hook-nudge-block", &server.url);
    with_project(&env);
    env.edit("s1", "src/a.rs", "x");
    env.edit("s1", "src/b.rs", "y");
    env.edit("s1", "src/a.rs", "z");
    let reason = blocked(&stop(&env, "s1", json!({}))).expect("a reminder");
    assert!(
        reason.starts_with("You changed 2 file(s). If you learned something reusable"),
        "{reason}"
    );
    assert!(reason.contains("memory_save"), "{reason}");
    assert!(
        blocked(&stop(&env, "s1", json!({}))).is_none(),
        "once per session"
    );
    assert!(blocked(&stop(&env, "s1", json!({"stop_hook_active": true}))).is_none());
}

#[test]
fn no_reminder_while_the_host_is_already_continuing() {
    let server = fake();
    let env = Env::new("hook-nudge-active", &server.url);
    with_project(&env);
    env.edit("s1", "src/a.rs", "x");
    assert!(blocked(&stop(&env, "s1", json!({"stop_hook_active": true}))).is_none());
}

#[test]
fn a_save_by_tool_or_cli_means_no_reminder() {
    let server = fake();
    let env = Env::new("hook-nudge-saved", &server.url);
    with_project(&env);
    env.edit("mcp", "src/a.rs", "x");
    env.run(
        "PostToolUse",
        &env.payload(
            "mcp",
            json!({"tool_name": "mcp__cairn__memory_save", "tool_input": {"kind": "gotcha"},
                   "tool_response": {"id":"mem_example", "status": "active", "deduplicated":false}}),
        ),
    );
    assert!(blocked(&stop(&env, "mcp", json!({}))).is_none());
    env.edit("cli", "src/a.rs", "x");
    env.bash(
        "cli",
        "/usr/local/bin/cairn save -",
        json!({"exit_code": 0}),
    );
    assert!(blocked(&stop(&env, "cli", json!({}))).is_none());
    // Another server's `memory_save` is not a Cairn save.
    env.edit("other", "src/a.rs", "x");
    env.run(
        "PostToolUse",
        &env.payload(
            "other",
            json!({"tool_name": "mcp__agentmemory__memory_save", "tool_input": {}}),
        ),
    );
    assert!(blocked(&stop(&env, "other", json!({}))).is_some());
}

#[test]
fn failed_saves_still_ask_on_stop_and_are_not_reported_as_saves() {
    let server = fake();
    let env = Env::new("hook-nudge-failed", &server.url);
    with_project(&env);
    for (session, response) in [
        (
            "mcp-error",
            json!({"isError":true,"content":[{"type":"text","text":"rejected"}]}),
        ),
        (
            "batch-error",
            json!({"content":[{"type":"text","text":"{\"results\":[{\"status\":\"error\"}]}"}]}),
        ),
        ("mcp-empty", json!({"results":[]})),
        (
            "shell-error",
            json!({"exit_code":1,"stderr":"save rejected"}),
        ),
    ] {
        env.edit(session, "src/a.rs", "x");
        if session == "shell-error" {
            env.bash(session, "cairn save -", response);
        } else {
            env.run("PostToolUse", &env.payload(session, json!({"tool_name":"mcp__cairn__memory_save","tool_input":{},"tool_response":response})));
        }
        assert!(
            blocked(&stop(&env, session, json!({}))).is_some(),
            "{session}"
        );
        env.run("SessionEnd", &env.payload(session, json!({})));
        let ends = server.to("end");
        assert_eq!(ends.last().unwrap().body["nudge_asked"], true);
        assert_eq!(ends.last().unwrap().body["nudge_saved"], false);
    }
}

#[test]
fn only_real_work_is_reminded() {
    let server = fake();
    let env = Env::new("hook-nudge-work", &server.url);
    with_project(&env);
    // Questions and reading: nothing.
    env.prompt("qa", "how does the queue work?");
    env.run(
        "PostToolUse",
        &env.payload(
            "qa",
            json!({"tool_name": "Read", "tool_input": {"file_path": "src/a.rs"}}),
        ),
    );
    assert!(blocked(&stop(&env, "qa", json!({}))).is_none());
    // A recognised test command with no edit does not ask, and a plain install does not.
    env.bash("inst", "bun install", json!({"exit_code": 0}));
    assert!(blocked(&stop(&env, "inst", json!({}))).is_none());
    env.bash("test", "bun test", json!({"exit_code": 0}));
    assert!(
        blocked(&stop(&env, "test", json!({}))).is_none(),
        "a test command with no edits does not ask"
    );
    // A Decision-style line without an edit does not ask.
    let said =
        json!({"last_assistant_message": "Done.\nDecision: use pgvector\nReason: one database"});
    assert!(blocked(&stop(&env, "decide", said)).is_none());
    assert!(
        blocked(&stop(
            &env,
            "plain",
            json!({"last_assistant_message": "All done."})
        ))
        .is_none()
    );
}

#[test]
fn no_project_means_no_reminder() {
    let server = fake();
    let env = Env::new("hook-nudge-noproject", &server.url);
    env.edit("s1", "src/a.rs", "x");
    assert!(blocked(&stop(&env, "s1", json!({}))).is_none());
}

#[test]
fn opencode_is_never_blocked() {
    let server = fake();
    let env = Env::new("hook-nudge-opencode", &server.url);
    with_project(&env);
    env.edit("s1", "src/a.rs", "x");
    let out = env.run_with(
        "Stop",
        &env.payload("s1", json!({})),
        &["--agent", "opencode"],
        &[],
    );
    assert!(blocked(&out).is_none());
    // The flag was not spent: a host that can continue still gets its reminder.
    assert!(blocked(&stop(&env, "s1", json!({}))).is_some());
}

#[test]
fn the_reminders_can_be_turned_off_by_config_or_environment() {
    let server = fake();
    let env = Env::new("hook-nudge-off", &server.url);
    with_project(&env);
    // Environment.
    env.edit("e", "src/a.rs", "x");
    let off = [("CAIRN_NO_NUDGE", "1")];
    let started = env.run_with("SessionStart", &env.payload("e", json!({})), &[], &off);
    assert_eq!(injected(&started).as_deref(), Some("BRIEF"));
    let stopped = env.run_with("Stop", &env.payload("e", json!({})), &[], &off);
    assert!(blocked(&stopped).is_none());
    // Config file.
    let mut config: Value =
        serde_json::from_str(&std::fs::read_to_string(env.config()).unwrap()).unwrap();
    config["nudge"] = json!(false);
    std::fs::write(env.config(), config.to_string()).unwrap();
    assert_eq!(start(&env, "c").as_deref(), Some("BRIEF"));
    env.edit("c", "src/a.rs", "x");
    assert!(blocked(&stop(&env, "c", json!({}))).is_none());
    // And on again once the key is gone.
    config.as_object_mut().unwrap().remove("nudge");
    std::fs::write(env.config(), config.to_string()).unwrap();
    assert!(
        start(&env, "d")
            .unwrap()
            .contains(DEFAULT_SESSION_INSTRUCTION)
    );
}

#[test]
fn server_instruction_texts_are_used_and_the_stop_text_needs_no_request() {
    let server = fake_with(|_, request| {
        if request.path.ends_with("/brief") {
            let reply = json!({"brief": "BRIEF", "instructions": {
                "session_instruction": "  Project rule: search Cairn first.  ",
                "stop_nudge": "Save to Cairn! Files: {files}. {files} again."}});
            return (200, reply.to_string());
        }
        default_reply(request)
    });
    let env = Env::new("hook-nudge-server", &server.url);
    with_project(&env);
    assert_eq!(
        start(&env, "s1").as_deref(),
        Some("BRIEF\nProject rule: search Cairn first.")
    );
    env.edit("s1", "src/a.rs", "x");
    env.edit("s1", "src/b.rs", "y");
    let before = server.requests().len();
    let reason = blocked(&stop(&env, "s1", json!({}))).unwrap();
    assert_eq!(reason, "Save to Cairn! Files: 2. 2 again.");
    let asked: Vec<String> = server.requests()[before..]
        .iter()
        .map(|r| r.path.clone())
        .collect();
    assert!(
        asked.iter().all(|p| !p.ends_with("/brief")),
        "no request for the text at Stop: {asked:?}"
    );
}

#[test]
fn empty_or_missing_server_text_and_an_unreachable_server_use_the_built_in_text() {
    let server = fake_with(|_, request| {
        if request.path.ends_with("/brief") {
            return (200, json!({"brief": "BRIEF", "instructions": {"session_instruction": "", "stop_nudge": "  "}}).to_string());
        }
        default_reply(request)
    });
    let env = Env::new("hook-nudge-empty-text", &server.url);
    with_project(&env);
    assert!(
        start(&env, "s1")
            .unwrap()
            .contains(DEFAULT_SESSION_INSTRUCTION)
    );
    env.edit("s1", "src/a.rs", "x");
    let reason = blocked(&stop(&env, "s1", json!({}))).unwrap();
    assert!(
        reason.starts_with("You changed 1 file(s). If you learned something reusable"),
        "{reason}"
    );
    // The server is not there at session start: the built-in line goes in alone.
    let env = Env::new("hook-nudge-down", "http://127.0.0.1:9");
    with_project(&env);
    let text = start(&env, "d1").unwrap();
    assert!(text == DEFAULT_SESSION_INSTRUCTION, "{text}");
    env.edit("d1", "src/a.rs", "x");
    let reason = blocked(&stop(&env, "d1", json!({}))).unwrap();
    assert!(reason.starts_with("You changed 1 file(s)"), "{reason}");
}

#[test]
fn no_project_start_prints_hint_once() {
    let server = fake();
    let env = Env::new("hook-no-project-hint", &server.url);
    let first = start(&env, "s1").unwrap();
    assert_eq!(first, brief_and_hint());
    assert!(NO_PROJECT_HINT.len() <= 90, "{}", NO_PROJECT_HINT.len());
    assert_eq!(
        start(&env, "s1").as_deref(),
        Some(format!("BRIEF\n{DEFAULT_SESSION_INSTRUCTION}").as_str()),
        "the hint is once per session"
    );
    let off = [("CAIRN_NO_NUDGE", "1")];
    let quiet = Env::new("hook-no-project-off", &server.url);
    let started = quiet.run_with("SessionStart", &quiet.payload("e", json!({})), &[], &off);
    assert_eq!(injected(&started).as_deref(), Some("BRIEF"));
    let down = Env::new("hook-no-project-down", "http://127.0.0.1:9");
    assert_eq!(
        start(&down, "d").as_deref(),
        Some(DEFAULT_SESSION_INSTRUCTION)
    );
}

#[test]
fn init_without_git_writes_local_repository() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "init-nogit");
    let dir = temp_dir("init-nogit").join("widget");
    std::fs::create_dir_all(&dir).unwrap();
    let output = team.owner.run_in(&dir, &["init", "Widget"]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{said}");
    let file: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(".cairn/project.json")).unwrap())
            .unwrap();
    assert_eq!(file["repository"], "local:widget");
    assert!(file["project_id"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(
        said.contains(
            "Keep .cairn/project.json with this folder; copies of the folder attach to the same project."
        ),
        "{said}"
    );
}

#[test]
fn init_suggests_commit_and_never_commits() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "init-git");
    let dir = checkout("init-suggest");
    let output = team.owner.run_in(&dir, &["init", "Demo"]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{said}");
    assert!(
        said.contains(
            "Commit .cairn/project.json so clones and worktrees attach: git add .cairn/project.json && git commit"
        ),
        "{said}"
    );
    let porcelain = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["status", "--porcelain"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(
        porcelain.contains("?? .cairn/"),
        "the file is untracked, not staged: {porcelain}"
    );
    assert!(
        porcelain.lines().all(|line| line.starts_with("??")),
        "nothing is staged: {porcelain}"
    );
    let commits = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["rev-list", "--count", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(commits.trim(), "1", "init does not commit");
}

#[test]
fn lookup_recorded_silently() {
    let server = fake();
    let env = Env::new("lookup-recorded", &server.url);
    let secret = format!("ghp_{}", "a".repeat(40));
    let output = env.run(
        "PostToolUse",
        &env.payload(
            "lookup",
            json!({
                "tool_name": "mcp__cairn__memory_search",
                "tool_input": {"query": format!("{secret} {}", "x".repeat(160))},
                "tool_response": {"results": [{"id": "mem_1"}]}
            }),
        ),
    );
    assert!(output.stdout.is_empty());
    let requests = server.to("lookup");
    assert_eq!(requests.len(), 1);
    let body = &requests[0].body;
    assert_eq!(body["kind"], "search");
    assert_eq!(body["result_ids"], json!(["mem_1"]));
    assert_eq!(body["hit"], true);
    let query = body["query"].as_str().unwrap();
    assert!(query.chars().count() <= 120 && !query.contains(&secret));
    for (response, expected) in [
        (
            json!({"content":[{"type":"text","text":"{\"memories\":[{\"id\":\"m\"}]}"}]}),
            json!(true),
        ),
        (json!({"memories":[]}), json!(false)),
        (json!({"isError":true}), Value::Null),
    ] {
        let output = env.run("PostToolUse", &env.payload("lookup", json!({
            "tool_name":"mcp__cairn__memory_get", "tool_input":{"ids":["m"]}, "tool_response":response
        })));
        assert!(output.stdout.is_empty());
        let requests = server.to("lookup");
        let body = &requests.last().unwrap().body;
        assert_eq!(body["query"], Value::Null);
        assert_eq!(body["result_ids"], json!(["m"]));
        assert_eq!(body["hit"], expected);
    }
}

#[test]
fn nudge_flags_sent() {
    let server = fake();
    let env = Env::new("nudge-flags", &server.url);
    env.edit("normal", "src/a.rs", "x");
    env.run("SessionEnd", &env.payload("normal", json!({})));
    let ends = server.to("end");
    assert_eq!(ends[0].body["nudge_asked"], false);
    assert_eq!(ends[0].body["nudge_saved"], false);
    env.run_with(
        "PostToolUse",
        &env.payload("auto", edit_call("src/b.rs", "x", "e")),
        &[],
        &[("CAIRN_SESSION_ORIGIN", "automation")],
    );
    env.run_with(
        "SessionEnd",
        &env.payload("auto", json!({})),
        &[],
        &[("CAIRN_SESSION_ORIGIN", "automation")],
    );
    assert!(server.to("end")[1].body.get("nudge_asked").is_none());
}

#[test]
fn failure_health_arrives_on_next_observe() {
    let server = fake_with(|_, request| {
        if request.path.ends_with("/lookup") {
            (401, "{}".into())
        } else {
            default_reply(request)
        }
    });
    let env = Env::new("lookup-health", &server.url);
    let output = env.run("PostToolUse", &env.payload("s", json!({
        "tool_name":"mcp__cairn__memory_search", "tool_input":{"query":"test"}, "tool_response":{"results":[]}
    })));
    assert!(output.stdout.is_empty());
    env.prompt("s", "a prompt");
    let seen = server.observed();
    assert_eq!(seen[0]["hook_error_class"], "auth");
    assert!(seen[0]["hook_error_at"].is_i64());
    assert_eq!(seen[0]["outbox_depth"], 0);
    env.prompt("s", "another prompt");
    assert!(server.observed()[1].get("hook_error_class").is_none());
}

#[test]
fn newer_hook_failure_survives_inflight_observe() {
    let started = Arc::new((Mutex::new(false), Condvar::new()));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let first = Arc::new(AtomicBool::new(true));
    let server = fake_with({
        let (started, release, first) = (started.clone(), release.clone(), first.clone());
        move |_, request| {
            if request.path.ends_with("/lookup") {
                return (401, "{}".into());
            }
            if request.path.ends_with("/observe") && first.swap(false, Ordering::SeqCst) {
                *started.0.lock().unwrap() = true;
                started.1.notify_one();
                let ready = release.0.lock().unwrap();
                drop(release.1.wait_while(ready, |ready| !*ready).unwrap());
            }
            default_reply(request)
        }
    });
    let env = Env::new("health-race", &server.url);
    let lookup = || {
        env.run("PostToolUse", &env.payload("race", json!({
        "tool_name":"mcp__cairn__memory_search", "tool_input":{"query":"test"}, "tool_response":{"results":[]}
    })))
    };
    lookup();
    thread::scope(|scope| {
        let send = scope.spawn(|| env.prompt("race", "first prompt"));
        let ready = started.0.lock().unwrap();
        drop(started.1.wait_while(ready, |ready| !*ready).unwrap());
        lookup();
        *release.0.lock().unwrap() = true;
        release.1.notify_one();
        send.join().unwrap();
    });
    env.prompt("race", "second prompt");
    let seen = server.observed();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1]["hook_error_class"], "auth");
}

#[test]
fn replay_409_does_not_record_hook_error() {
    let server = fake_with(|_, request| {
        if request.body["event_id"] == "hk_duplicate" {
            (409, "{}".to_owned())
        } else {
            default_reply(request)
        }
    });
    let env = Env::new("hook-replay-409-health", &server.url);
    write_outbox(
        &env.outbox(),
        &server.url,
        TOKEN,
        &["hk_duplicate".to_owned()],
    );
    env.prompt("s1", "trigger replay");
    assert!(queued_ids(&env.outbox()).is_empty());
    env.prompt("s1", "next capture");
    let observed = server.observed();
    assert_eq!(observed.len(), 3);
    assert!(
        observed[2].get("hook_error_class").is_none(),
        "{}",
        observed[2]
    );
}

#[test]
fn renamed_project_notice_appears_once_per_session() {
    let server = fake_with(|_, request| {
        if request.path.ends_with("/brief") {
            return (
                200,
                json!({"brief":"BRIEF", "project_id":"proj_test", "project_name":"New"})
                    .to_string(),
            );
        }
        default_reply(request)
    });
    let env = Env::new("renamed-notice", &server.url);
    std::fs::create_dir_all(env.repo.join(".cairn")).unwrap();
    let path = env.repo.join(".cairn/project.json");
    let original = r#"{"project_id":"proj_test","name":"Old"}"#;
    std::fs::write(&path, original).unwrap();
    let first = injected(&env.run("SessionStart", &env.payload("rename", json!({})))).unwrap();
    assert!(first.contains("Cairn: this project was renamed to \"New\"; run `cairn init` and commit .cairn/project.json."));
    let second = injected(&env.run("SessionStart", &env.payload("rename", json!({})))).unwrap();
    assert!(!second.contains("was renamed"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    std::fs::write(&path, r#"{"project_id":"proj_test","name":"New"}"#).unwrap();
    let matching = injected(&env.run("SessionStart", &env.payload("matching", json!({})))).unwrap();
    assert!(!matching.contains("was renamed"));
}

#[test]
fn init_updates_only_the_renamed_project_name() {
    let server = fake_with(|_, request| {
        if request.path.ends_with("/init") {
            return (
                200,
                json!({"project_id":"proj_test", "name":"New", "created":false}).to_string(),
            );
        }
        default_reply(request)
    });
    let env = Env::new("init-rename", &server.url);
    std::fs::create_dir_all(env.repo.join(".cairn")).unwrap();
    let path = env.repo.join(".cairn/project.json");
    let original = "{\n \"name\":\"Old\", \"extra\": 3, \"project_id\": \"proj_test\"\n}\n";
    std::fs::write(&path, original).unwrap();
    let output = command(&env.home)
        .current_dir(&env.repo)
        .arg("init")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(String::from_utf8_lossy(&output.stdout).contains(
        "Project renamed to \"New\" in the web; updated .cairn/project.json — commit it."
    ));
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        original.replace("\"Old\"", "\"New\"")
    );
}

#[test]
fn rename_notice_marks_a_long_name_as_truncated() {
    let server = fake_with(|_, request| {
        if request.path.ends_with("/brief") {
            return (200, json!({"project_id":"proj_test", "project_name":"A very long project name beyond the limit"}).to_string());
        }
        default_reply(request)
    });
    let env = Env::new("rename-long", &server.url);
    std::fs::create_dir_all(env.repo.join(".cairn")).unwrap();
    std::fs::write(
        env.repo.join(".cairn/project.json"),
        r#"{"project_id":"proj_test","name":"Old"}"#,
    )
    .unwrap();
    let text = injected(&env.run("SessionStart", &env.payload("s", json!({})))).unwrap();
    let notice = text
        .lines()
        .find(|line| line.contains("was renamed"))
        .unwrap();
    assert!(notice.contains('…'));
    assert!(notice.len() <= 120, "{notice}");
}
