//! Runs the real `cairn` binary (CLI and `cairn mcp`) against an in-process Cairn server on a
//! `TestDb` schema. Web-side setup (accounts, devices) uses a session token, as the web console
//! does; the agent side only ever holds a device token.

use cairn_client::{Client, Config, Error};
use cairn_server::{AppState, router};
use cairn_store::testing::TestDb;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;

const PASSWORD: &str = "correct horse battery staple";

/// A server on a random port over a fresh test schema. Dropping it stops the server and drops
/// the schema.
struct Server {
    url: String,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    /// `None` when no test database is configured (the test is skipped).
    fn start() -> Option<Self> {
        let (sender, receiver) = mpsc::channel();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async {
                let Some(db) = TestDb::new().await else {
                    sender.send(None).unwrap();
                    return;
                };
                let app = router(AppState::new(db.store.clone(), None));
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                sender.send(Some(listener.local_addr().unwrap())).unwrap();
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = stopped.await;
                    })
                    .await
                    .unwrap();
                db.finish().await;
            });
        });
        match receiver.recv().unwrap() {
            Some(addr) => Some(Self {
                url: format!("http://{addr}"),
                stop: Some(stop),
                thread: Some(thread),
            }),
            None => {
                thread.join().unwrap();
                None
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cairn-agent-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn web(url: &str, token: &str) -> Client {
    Client::new(Config::new(url, token).unwrap())
}

fn command(config_home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cairn"));
    command
        .env("XDG_CONFIG_HOME", config_home)
        .env_remove("CAIRN_URL")
        .env_remove("CAIRN_DEVICE_TOKEN")
        .env_remove("CAIRN_CONFIG")
        .env("CAIRN_NO_UPDATE_CHECK", "1")
        // `setup` fetches the hook scripts; keep tests off the network (refused at once).
        .env("CAIRN_UPDATE_URL", "http://127.0.0.1:1/releases/latest");
    command
}

/// Runs `command` with `input` as its whole stdin.
fn with_stdin(command: &mut Command, input: &str) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A command that rejects its arguments exits without reading stdin: ignore the broken pipe.
    let _ = writeln!(child.stdin.take().unwrap(), "{input}");
    child.wait_with_output().unwrap()
}

/// One agent: its own config home and device token.
struct Agent {
    home: PathBuf,
    token: String,
}

impl Agent {
    fn setup(name: &str, url: &str, token: &str) -> Self {
        let home = temp_dir(name);
        let output = with_stdin(command(&home).args(["setup", url]), token);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains(token));
        Self {
            home,
            token: token.to_owned(),
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        command(&self.home).args(args).output().unwrap()
    }

    fn run_in(&self, dir: &Path, args: &[&str]) -> Output {
        command(&self.home)
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let output = self.run(args);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{args:?}: {stderr}");
        assert!(!stderr.contains(&self.token));
        serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
    }

    fn mcp(&self) -> Mcp {
        let mut child = command(&self.home)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut mcp = Mcp {
            child,
            stdin,
            stdout,
            next_id: 1,
        };
        mcp.request("initialize", json!({"protocolVersion": "2025-06-18"}));
        writeln!(
            mcp.stdin,
            r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
        )
        .unwrap();
        mcp
    }
}

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Mcp {
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{line}").unwrap();
        let mut response = String::new();
        self.stdout.read_line(&mut response).unwrap();
        serde_json::from_str(&response).unwrap()
    }

    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))["result"].clone()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn device_token(web: &Client, name: &str) -> (String, String) {
    let registered = web
        .post("/cairn-memory/devices/register", &json!({"name": name}))
        .unwrap();
    (
        registered["token"].as_str().unwrap().to_owned(),
        registered["device"]["device_id"]
            .as_str()
            .unwrap()
            .to_owned(),
    )
}

/// The name the server holds for one of the agent's projects (the list is newest first).
fn project_name(agent: &Agent, project_id: &str) -> String {
    agent
        .json(&["project-list"])
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["project_id"] == project_id)
        .unwrap()["name"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn scenario(url: &str, label: &str) {
    // Web side: first account, a second account, and one device token each.
    let anonymous = web(url, "none");
    let owner = anonymous
        .post(
            "/cairn-memory/auth/bootstrap",
            &json!({"email": "owner@example.test", "password": PASSWORD}),
        )
        .unwrap();
    let owner_web = web(url, owner["token"].as_str().unwrap());
    // Only the admin session creates accounts.
    owner_web
        .post(
            "/cairn-memory/auth/register",
            &json!({"email": "member@example.test", "password": PASSWORD}),
        )
        .unwrap();
    let member = anonymous
        .post(
            "/cairn-memory/auth/login",
            &json!({"email": "member@example.test", "password": PASSWORD}),
        )
        .unwrap();
    let member_web = web(url, member["token"].as_str().unwrap());
    let (owner_token, owner_device) = device_token(&owner_web, "owner agent");
    let (member_token, _) = device_token(&member_web, "member agent");
    let agent = Agent::setup(&format!("{label}-owner"), url, &owner_token);
    let teammate = Agent::setup(&format!("{label}-member"), url, &member_token);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let config = agent.home.join("cairn").join("config.json");
        assert_eq!(
            std::fs::metadata(config).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    // Capture, session end, and recall.
    let project = agent.json(&[
        "project-register",
        "example.test/acme/cairn-demo",
        "Cairn Demo",
    ]);
    let project_id = project["project_id"].as_str().unwrap().to_owned();
    assert_eq!(project_name(&agent, &project_id), "Cairn Demo");

    agent.json(&["start", "private-session"]);
    agent.json(&[
        "capture",
        "private-session",
        "evt-private",
        "private observation stays on the owner account",
    ]);
    let private = agent.json(&["end", "private-session"]);
    assert_eq!(private["status"], "queued");
    // The raw capture is never recalled, only what is distilled from it later.
    let private_id = agent.json(&[
        "save",
        r#"{"scope":"personal","kind":"note","title":"Private note","summary":"stays on the owner account","reason":"flow"}"#,
    ])["id"]
        .as_str()
        .unwrap()
        .to_owned();

    agent.json(&[
        "start",
        "shared-session",
        "example.test/acme/cairn-demo",
        "",
        "",
        "abc1234",
    ]);
    let captured = agent.json(&[
        "capture",
        "shared-session",
        "evt-shared",
        "repository cache decision is ready for the project",
    ]);
    assert_eq!(captured["duplicate"], false);
    let replay = agent.json(&[
        "capture",
        "shared-session",
        "evt-shared",
        "repository cache decision is ready for the project",
    ]);
    assert_eq!(replay["duplicate"], true);
    let ended = agent.json(&["end", "shared-session"]);
    assert_eq!(ended["status"], "queued");
    let lesson = agent.json(&["lesson", r#"{"scope":"personal","title":"Credential handling","summary":"Keep device tokens out of logs.","reason":"Avoid exposure","fields":{"fix":"redact"}}"#]);
    let lesson_id = lesson["id"].as_str().unwrap().to_owned();
    assert_eq!(agent.json(&["memory", &lesson_id])["kind"], "gotcha");

    let record = json!({"scope":"project","kind":"decision","title":"repository cache decision","summary":"Ready for the project","reason":"Agent flow verification","context":{"project_id":project_id,"repository":"example.test/acme/cairn-demo"}}).to_string();
    let promoted = agent.json(&["save", &record]);
    let promoted_id = promoted["id"].as_str().unwrap().to_owned();
    assert_eq!(agent.json(&["memory", &promoted_id])["scope"], "project");
    let context = agent.json(&[
        "context",
        "cache decision",
        "5",
        "example.test/acme/cairn-demo",
    ]);
    assert!(context.to_string().contains("cache decision"));
    // An unknown key is a clear 400, not a silent drop.
    let bad = agent.run(&[
        "save",
        r#"{"kind":"rule","title":"t","summary":"s","reason":"r","colour":"red"}"#,
    ]);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("colour"));
    // Membership comes from the committed project file: the owner runs `cairn init` in the
    // checkout, the teammate's first capture there joins the project. There is no invite.
    let checkout = temp_dir(&format!("{label}-checkout"));
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&checkout)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    };
    git(&["init", "-q"]);
    git(&[
        "remote",
        "add",
        "origin",
        "https://example.test/acme/cairn-demo.git",
    ]);
    let not_git = agent.run_in(&temp_dir(&format!("{label}-plain")), &["init"]);
    assert!(!not_git.status.success());
    let init = agent.run_in(&checkout, &["init"]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    assert!(String::from_utf8_lossy(&init.stdout).contains("Commit .cairn/project.json"));
    let file = checkout.join(".cairn/project.json");
    let written: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(
        written,
        json!({"project_id": project_id, "repository": "example.test/acme/cairn-demo", "name": "Cairn Demo"})
    );
    assert!(
        agent.run_in(&checkout, &["init"]).status.success(),
        "init repeats"
    );
    let conflict = agent.run_in(&checkout, &["init", "Different name"]);
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("--rename"));
    assert_eq!(
        serde_json::from_str::<Value>(&std::fs::read_to_string(&file).unwrap()).unwrap(),
        written
    );

    // Legacy files acquire the server's name; readers still accept the old shape.
    let mut legacy = written.clone();
    legacy.as_object_mut().unwrap().remove("name");
    std::fs::write(&file, legacy.to_string()).unwrap();
    assert!(agent.run_in(&checkout, &["init"]).status.success());
    assert_eq!(
        serde_json::from_str::<Value>(&std::fs::read_to_string(&file).unwrap()).unwrap(),
        written
    );

    // A changed server name is explicit, and a stale checkout must ask for repair.
    let rename_path = format!("/cairn-memory/projects/{project_id}/rename");
    owner_web
        .post(&rename_path, &json!({"name": "Renamed remotely"}))
        .unwrap();
    let stale = agent.run_in(&checkout, &["init"]);
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("--rename"));
    assert!(
        agent
            .run_in(&checkout, &["init", "--rename", "Cairn Demo"])
            .status
            .success()
    );
    assert_eq!(project_name(&agent, &project_id), "Cairn Demo");
    assert!(
        owner_web
            .get("/cairn-memory/auth/audit")
            .unwrap()
            .to_string()
            .contains("project_renamed")
    );

    let fresh = temp_dir(&format!("{label}-fresh-name"));
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&fresh)
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        agent
            .run_in(&fresh, &["init", "Fresh name"])
            .status
            .success()
    );
    let fresh_file: Value =
        serde_json::from_slice(&std::fs::read(fresh.join(".cairn/project.json")).unwrap()).unwrap();
    assert_eq!(fresh_file["name"], "Fresh name");
    assert!(fresh_file["project_id"].is_string());

    std::fs::write(
        &file,
        r#"{"project_id": "project-other", "repository": "example.test/acme/cairn-demo"}"#,
    )
    .unwrap();
    let refused = agent.run_in(&checkout, &["init"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("project not found"));
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains("project-other")
    );
    std::fs::write(&file, serde_json::to_string(&written).unwrap()).unwrap();
    assert_eq!(teammate.json(&["project-list"]), json!([]));
    let refused = teammate.run_in(&checkout, &["init", "--rename", "Outsider rename"]);
    assert!(!refused.status.success());
    assert_eq!(project_name(&agent, &project_id), "Cairn Demo");

    let joined = teammate.run_in(
        &checkout,
        &[
            "capture",
            "join-session",
            "evt-join",
            "joined by the project file",
        ],
    );
    assert!(
        joined.status.success(),
        "{}",
        String::from_utf8_lossy(&joined.stderr)
    );
    assert_eq!(
        teammate.json(&["project-list"])[0]["project_id"],
        project_id.as_str()
    );

    let refused = teammate.run_in(&checkout, &["init", "--rename", "Member rename"]);
    assert!(!refused.status.success());
    assert_eq!(
        serde_json::from_str::<Value>(&std::fs::read_to_string(&file).unwrap()).unwrap(),
        written
    );
    let shared = teammate.json(&["project-memories", &project_id]);
    let ids: Vec<_> = shared
        .as_array()
        .unwrap()
        .iter()
        .map(|memory| memory["id"].clone())
        .collect();
    assert!(ids.contains(&json!(promoted_id)));
    assert!(!shared.to_string().contains(&private_id));
    assert_eq!(
        agent
            .json(&["project-memories", &project_id])
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // The member's device cannot act as the owner.
    let refused = teammate.run(&["project-team-create", &project_id, "Other"]);
    assert!(!refused.status.success());

    // Export writes a file; forget blanks the memory, which then leaves the export.
    let bundle = agent.home.join("bundle.json");
    agent.json(&["export", bundle.to_str().unwrap()]);
    let exported = std::fs::read_to_string(&bundle).unwrap();
    assert!(exported.contains("repository cache decision") && exported.contains("Private note"));
    agent.json(&["forget", &private_id]);
    agent.json(&["export", bundle.to_str().unwrap()]);
    assert!(
        !std::fs::read_to_string(&bundle)
            .unwrap()
            .contains("Private note")
    );
    assert!(
        !agent
            .run(&["import", bundle.to_str().unwrap()])
            .status
            .success()
    );
    assert!(
        !agent
            .run(&["correct", &promoted_id, "text"])
            .status
            .success()
    );

    // MCP proxies the same calls with the same device token.
    let mut mcp = agent.mcp();
    let tools = mcp.request("tools/list", json!({}));
    let names: Vec<_> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"memory_observe"));
    for removed in [
        "memory_project_invite",
        "memory_project_team_add",
        "memory_project_share",
        "memory_project_shared",
        "memory_project_revoke",
        "memory_project_team",
        "memory_project_team_create",
        "memory_correct",
        "memory_import",
        "memory_similarity_reviews",
        "memory_review_dismiss",
        "memory_review_contradict",
        "memory_review_supersede",
        "memory_project_verify",
        "memory_project_freshness",
    ] {
        assert!(!names.contains(&removed), "{removed}");
    }
    assert_eq!(
        mcp.tool("memory_session_start", json!({"sessionId": "mcp-session"}))["isError"],
        false
    );
    let observed = mcp.tool(
        "memory_observe",
        json!({"eventId": "evt-mcp", "sessionId": "mcp-session", "content": "mcp proxied observation", "role": "agent", "source": "mcp"}),
    );
    assert_eq!(observed["structuredContent"]["accepted"], true);
    let ended = mcp.tool("memory_session_end", json!({"sessionId": "mcp-session"}));
    assert_eq!(ended["isError"], false);
    let saved = mcp.tool(
        "memory_save",
        json!({"kind": "fact", "scope": "personal", "title": "Proxied fact", "summary": "The proxy keeps the device token.", "reason": "mcp flow", "fields": {"source": "agent_flow"}}),
    );
    assert_eq!(saved["structuredContent"]["status"], "active", "{saved}");
    let saved_id = saved["structuredContent"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let recalled = mcp.tool(
        "memory_recall",
        json!({"query": "proxied fact device token"}),
    );
    assert_eq!(
        recalled["structuredContent"]["results"][0]["id"],
        saved_id.as_str()
    );
    assert!(
        recalled["structuredContent"]["results"][0]["kind"]
            .as_str()
            .is_some()
    );
    let expanded = mcp.tool(
        "memory_recall",
        json!({"query": "proxied fact device token", "expanded": true}),
    );
    let expanded_text = expanded["content"][0]["text"].as_str().unwrap();
    assert!(expanded_text.contains("agent_flow"), "{expanded_text}");
    for key in ["usage", "evidence_hash", "identity_hash", "owner"] {
        assert!(
            expanded["structuredContent"]["results"][0]
                .get(key)
                .is_none(),
            "{key}"
        );
    }
    let listed = mcp.tool("memory_project_list", json!({}));
    assert!(
        listed["structuredContent"]["projects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["project_id"] == project_id.as_str())
    );
    let missing = mcp.tool("memory_save", json!({"scope":"project"}));
    assert_eq!(missing["isError"], true);
    assert!(!listed.to_string().contains(&owner_token));
    drop(mcp);

    // Web-only actions refuse a device token with 403 and the server's own message.
    let device = web(url, &owner_token);
    for result in [
        device.get("/cairn-memory/auth/audit"),
        device.get("/cairn-memory/devices"),
    ] {
        match result {
            Err(Error::Status {
                status: 403,
                message,
            }) => {
                assert!(
                    message.contains("device tokens cannot do this"),
                    "{message}"
                );
            }
            other => panic!("expected 403, got {other:?}"),
        }
    }

    // Revoking the device in the web console locks the agent out.
    owner_web
        .post(
            "/cairn-memory/devices/revoke",
            &json!({"deviceId": owner_device}),
        )
        .unwrap();
    let locked = agent.run(&["project-list"]);
    assert!(!locked.status.success());
    let config = agent.home.join("cairn").join("config.json");
    let stderr = String::from_utf8_lossy(&locked.stderr);
    assert!(
        stderr.contains("HTTP 401")
            && stderr.contains(&config.display().to_string())
            && stderr.contains("cairn setup <server-url>")
            && !stderr.contains(&owner_token)
            && !stderr.contains(&agent.token),
        "{stderr}"
    );
    let mut mcp = agent.mcp();
    let failed = mcp.tool("memory_project_list", json!({}));
    let text = failed.to_string();
    assert_eq!(failed["isError"], true, "{text}");
    assert!(
        text.contains("HTTP 401")
            && text.contains(&config.display().to_string())
            && text.contains("cairn setup <server-url>")
            && !text.contains(&owner_token)
            && !text.contains(&agent.token),
        "{text}"
    );
}

#[test]
fn agent_flow_on_the_postgres_server() {
    let Some(server) = Server::start() else {
        return;
    };
    scenario(&server.url, "postgres");
}

#[test]
fn setup_refuses_plain_http_off_loopback_and_legacy_env_is_not_configuration() {
    let home = temp_dir("setup-refusal");
    let output = with_stdin(
        command(&home).args(["setup", "http://cairn.example.test"]),
        "cdt_secret_value",
    );
    assert!(!output.status.success());
    assert!(!home.join("cairn").join("config.json").exists());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("cdt_secret_value"));

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let output = command(&home)
        .env("CAIRN_URL", &url)
        .env("CAIRN_DEVICE_TOKEN", "cdt_wrong_value")
        .arg("project-list")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("not configured"), "{stderr}");
    assert!(
        stderr.contains("ignoring CAIRN_URL and CAIRN_DEVICE_TOKEN"),
        "{stderr}"
    );
    assert!(!stderr.contains("cdt_wrong_value"), "{stderr}");
    assert!(!stderr.contains(&url), "{stderr}");
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn setup_takes_no_token_argument_and_one_legacy_variable_is_not_configuration() {
    let home = temp_dir("setup-argv");
    let output = with_stdin(
        command(&home).args(["setup", "https://cairn.example.test", "cdt_in_argv"]),
        "cdt_secret_value",
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(!home.join("cairn").join("config.json").exists());

    let output = with_stdin(
        command(&home).args(["setup", "https://cairn.example.test"]),
        "",
    );
    assert!(!output.status.success(), "an empty token is refused");

    let output = command(&home)
        .env("CAIRN_URL", "https://other.example.test")
        .arg("project-list")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("ignoring CAIRN_URL"), "{stderr}");
    assert!(stderr.contains("not configured"), "{stderr}");
    assert!(!stderr.contains("https://other.example.test"), "{stderr}");
    assert!(!stderr.contains("set both"), "{stderr}");

    for (name, value) in [("CAIRN_URL", ""), ("CAIRN_DEVICE_TOKEN", "   ")] {
        let output = command(&home)
            .env(name, value)
            .arg("project-list")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{name}: {stderr}");
        assert!(
            stderr.contains(&format!("ignoring {name}")),
            "{name}: {stderr}"
        );
        assert!(stderr.contains("not configured"), "{name}: {stderr}");
        assert!(
            !stderr.contains("https://other.example.test"),
            "{name}: {stderr}"
        );
    }
}

/// Accepts a few HTTP requests and answers each with `response`.
fn serve_bytes(response: String) -> (String, mpsc::Receiver<String>) {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for _ in 0..8 {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buffer = [0u8; 4096];
            let read = stream.read(&mut buffer).unwrap_or(0);
            let _ = sender.send(String::from_utf8_lossy(&buffer[..read]).into_owned());
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (url, receiver)
}

#[test]
fn selected_config_file_wins_over_legacy_env_for_cli_and_mcp() {
    use std::net::TcpListener;
    use std::time::Duration;
    let file_token = "cdt_file_token_value";
    let env_token = "cdt_env_token_value";
    let default_token = "cdt_default_token_value";
    let (url, seen) = serve_bytes("HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n[]".to_owned());
    let wrong = TcpListener::bind("127.0.0.1:0").unwrap();
    let wrong_url = format!("http://{}", wrong.local_addr().unwrap());
    drop(wrong);
    let home = temp_dir("selected-config");
    let picked = home.join("picked.json");
    std::fs::write(
        &picked,
        json!({"url": &url, "token": file_token}).to_string(),
    )
    .unwrap();
    let default_dir = home.join("cairn");
    std::fs::create_dir_all(&default_dir).unwrap();
    std::fs::write(
        default_dir.join("config.json"),
        json!({"url": &wrong_url, "token": default_token}).to_string(),
    )
    .unwrap();

    let setup = with_stdin(
        command(&home)
            .env("CAIRN_CONFIG", &picked)
            .args(["setup", "https://cairn.example.test"]),
        "cdt_setup_token_value",
    );
    assert!(
        setup.status.success(),
        "{}",
        String::from_utf8_lossy(&setup.stderr)
    );
    let stored = std::fs::read_to_string(&picked).unwrap();
    assert!(stored.contains("https://cairn.example.test"), "{stored}");
    assert!(!stored.contains(file_token));
    assert!(!String::from_utf8_lossy(&setup.stdout).contains("cdt_setup_token_value"));
    assert!(!String::from_utf8_lossy(&setup.stderr).contains("cdt_setup_token_value"));
    std::fs::write(
        &picked,
        json!({"url": &url, "token": file_token}).to_string(),
    )
    .unwrap();

    for (label, url_env, token_env) in [
        ("both", Some(wrong_url.as_str()), Some(env_token)),
        ("url-only", Some(wrong_url.as_str()), None),
        ("token-only", None, Some(env_token)),
    ] {
        let mut run = command(&home);
        run.env("CAIRN_CONFIG", &picked).arg("project-list");
        match url_env {
            Some(value) => {
                run.env("CAIRN_URL", value);
            }
            None => {
                run.env_remove("CAIRN_URL");
            }
        }
        match token_env {
            Some(value) => {
                run.env("CAIRN_DEVICE_TOKEN", value);
            }
            None => {
                run.env_remove("CAIRN_DEVICE_TOKEN");
            }
        }
        let output = run.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{label}: {stderr}");
        assert!(stderr.contains("ignoring"), "{label}: {stderr}");
        assert!(!stderr.contains(file_token), "{label}");
        assert!(!stderr.contains(env_token), "{label}");
        assert!(!stderr.contains(default_token), "{label}");
        assert!(!stderr.contains(&wrong_url), "{label}: {stderr}");
        let head = seen.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            head.contains(&format!("Bearer {file_token}")),
            "{label}: {head}"
        );
    }

    let default_only = command(&home).arg("project-list").output().unwrap();
    let stderr = String::from_utf8_lossy(&default_only.stderr);
    assert!(
        !default_only.status.success(),
        "the decoy default URL is not a server: {stderr}"
    );
    assert!(
        !stderr.contains("ignoring"),
        "no legacy variable is set: {stderr}"
    );
    std::fs::write(
        default_dir.join("config.json"),
        json!({"url": &url, "token": default_token}).to_string(),
    )
    .unwrap();
    let output = command(&home).arg("project-list").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let head = seen.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(head.contains(&format!("Bearer {default_token}")), "{head}");

    let body = format!(r#"{{"error":"rejected {file_token}"}}"#);
    let (denied_url, _) = serve_bytes(format!(
        "HTTP/1.1 401 Unauthorized\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    ));
    std::fs::write(
        &picked,
        json!({"url": denied_url, "token": file_token}).to_string(),
    )
    .unwrap();
    let mut child = command(&home)
        .env("CAIRN_CONFIG", &picked)
        .env("CAIRN_URL", &wrong_url)
        .env("CAIRN_DEVICE_TOKEN", env_token)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let write = |stdin: &mut ChildStdin, line: &str| {
        writeln!(stdin, "{line}").unwrap();
    };
    write(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
    );
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let initialize: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(initialize["id"], 1, "{line}");
    write(
        &mut stdin,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );
    write(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memory_project_list","arguments":{}}}"#,
    );
    line.clear();
    stdout.read_line(&mut line).unwrap();
    let reply: Value = serde_json::from_str(line.trim()).unwrap();
    let text = reply.to_string();
    assert_eq!(reply["id"], 2, "{text}");
    assert_eq!(reply["result"]["isError"], true, "{text}");
    assert!(text.contains("HTTP 401"), "{text}");
    assert!(text.contains(&picked.display().to_string()), "{text}");
    assert!(text.contains("cairn setup <server-url>"), "{text}");
    assert!(!text.contains(file_token), "{text}");
    assert!(!text.contains(env_token), "{text}");
    assert!(!line.contains('\n') || line.ends_with('\n'));
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ignoring CAIRN_URL and CAIRN_DEVICE_TOKEN"),
        "{stderr}"
    );
    assert!(!stderr.contains("jsonrpc"), "{stderr}");
    assert!(!stderr.contains(file_token), "{stderr}");
    assert!(!stderr.contains(env_token), "{stderr}");
    assert!(!stderr.contains(&wrong_url), "{stderr}");
}

#[cfg(unix)]
#[test]
fn export_is_private_and_replaces_a_symlink_instead_of_following_it() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let Some(server) = Server::start() else {
        return;
    };
    let url = server.url.clone();
    let anonymous = web(&url, "none");
    let owner = anonymous
        .post(
            "/cairn-memory/auth/bootstrap",
            &json!({"email": "owner@example.test", "password": PASSWORD}),
        )
        .unwrap();
    let owner_web = web(&url, owner["token"].as_str().unwrap());
    let (token, _) = device_token(&owner_web, "export agent");
    let agent = Agent::setup("export", &url, &token);

    let output = agent.home.join("bundle.json");
    agent.json(&["export", output.to_str().unwrap()]);
    assert_eq!(
        std::fs::metadata(&output).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let victim = agent.home.join("victim.txt");
    std::fs::write(&victim, "keep").unwrap();
    std::fs::remove_file(&output).unwrap();
    symlink(&victim, &output).unwrap();
    agent.json(&["export", output.to_str().unwrap()]);
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
    assert!(
        !std::fs::symlink_metadata(&output)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn a_loopback_token_never_goes_through_an_environment_proxy() {
    use std::net::TcpListener;
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    proxy.set_nonblocking(true).unwrap();
    let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", target.local_addr().unwrap());
    let served = thread::spawn(move || {
        let (mut stream, _) = target.accept().unwrap();
        let mut buffer = [0u8; 4096];
        let read = std::io::Read::read(&mut stream, &mut buffer).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n[]")
            .unwrap();
        String::from_utf8_lossy(&buffer[..read]).into_owned()
    });
    let home = temp_dir("proxy");
    let mut run = command(&home);
    for name in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        run.env(name, &proxy_url);
    }
    let config_dir = home.join("cairn");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.json"),
        json!({"url": &url, "token": "cdt_proxy_probe"}).to_string(),
    )
    .unwrap();
    let output = run.arg("project-list").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(served.join().unwrap().contains("Bearer cdt_proxy_probe"));
    assert_eq!(
        proxy.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "the proxy must not see the request"
    );
}

#[test]
fn help_and_flag_errors_never_make_a_request() {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let home = temp_dir("help-no-request");
    let checkout = temp_dir("help-checkout");
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&checkout)
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success()
    );
    let commands = [
        "init",
        "setup",
        "mcp",
        "update",
        "version",
        "start",
        "capture",
        "end",
        "search",
        "context",
        "why-changed",
        "resume",
        "unresolved",
        "forget",
        "lesson",
        "project-register",
        "project-list",
        "save",
        "memories",
        "memory",
        "project-members",
        "project-memories",
        "delete-memory",
        "export",
    ];
    let invoke = |args: &[&str]| {
        command(&home)
            .current_dir(&checkout)
            .env("CAIRN_URL", &url)
            .env("CAIRN_DEVICE_TOKEN", "cdt_no_request")
            .env("CAIRN_UPDATE_URL", format!("{url}/latest"))
            .stdin(Stdio::null())
            .args(args)
            .output()
            .unwrap()
    };
    for args in [vec!["--help"], vec!["-h"], vec!["help", "init"]] {
        let output = invoke(&args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
    }
    for name in commands {
        for flag in ["--help", "-h"] {
            let output = invoke(&[name, flag]);
            assert!(
                output.status.success(),
                "{name} {flag}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
        }
        let output = invoke(&[name, "--bogus"]);
        assert!(!output.status.success(), "{name}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unexpected option"),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for args in [
        vec!["init", "--rename", "--bogus"],
        vec!["project-register", "repo", "--bogus"],
        vec!["capture", "s", "e", "--bogus"],
    ] {
        let output = invoke(&args);
        assert!(!output.status.success(), "{args:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("unexpected option"));
    }
    std::fs::create_dir(checkout.join(".cairn")).unwrap();
    let file = checkout.join(".cairn/project.json");
    let original = r#"{"project_id":"project-local","repository":"repo","name":"Recorded"}"#;
    std::fs::write(&file, original).unwrap();
    let output = invoke(&["init", "Different"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--rename"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
#[cfg(unix)]
fn agent_setup_is_idempotent_removable_and_preserves_unrelated_config() {
    for agent in ["claude", "codex", "opencode"] {
        let home = temp_dir(&format!("agent-setup-{agent}"));
        let config_home = home.join(".config");
        let bin = home.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for name in ["codex", "opencode"] {
            let path = bin.join(name);
            std::fs::write(&path, include_bytes!("fixtures/native_mcp.py")).unwrap();
            #[cfg(unix)]
            std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
        }
        let path = std::env::join_paths(
            std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let run = |remove: bool| {
            let mut cmd = command(&config_home);
            cmd.env("HOME", &home)
                .env("PATH", &path)
                .env("USERPROFILE", &home)
                .env_remove("CODEX_HOME")
                .env_remove("CLAUDE_CONFIG_DIR");
            cmd.args(["setup", "https://unused.example.test", "--agent", agent]);
            if remove {
                cmd.arg("--remove");
            }
            let output = with_stdin(&mut cmd, "test-device");
            assert!(
                output.status.success(),
                "{agent}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(false);
        let files = match agent {
            "claude" => vec![
                home.join(".claude/settings.json"),
                home.join(".claude.json"),
            ],
            "codex" => vec![
                home.join(".codex/hooks.json"),
                home.join(".codex/config.toml"),
            ],
            _ => vec![
                config_home.join("opencode/AGENTS.md"),
                config_home.join("opencode/opencode.json"),
            ],
        };
        let first: Vec<_> = files.iter().map(|p| std::fs::read(p).unwrap()).collect();
        run(false);
        for (file, expected) in files.iter().zip(&first) {
            assert_eq!(&std::fs::read(file).unwrap(), expected);
        }
        run(true);
        run(true);
        for file in files {
            let text = std::fs::read_to_string(file).unwrap();
            assert!(!text.contains("--cairn-managed"));
            assert!(!text.contains("CAIRN_SETUP_MANAGED"));
            assert!(!text.contains("<!-- cairn:begin -->"));
        }
        std::fs::remove_dir_all(home).unwrap();
    }
}
