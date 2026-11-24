//! Shared harness: the real `cairn` binary against an in-process Cairn server on a `TestDb`
//! schema. Web-side setup (accounts, devices) uses a session token, as the web console does;
//! the agent side only ever holds a device token.
#![allow(dead_code)]

use cairn_client::{Client, Config};
use cairn_server::{AppState, router};
use cairn_store::Store;
use cairn_store::testing::TestDb;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;

pub const PASSWORD: &str = "correct horse battery staple";

/// A server on a random port over a fresh test schema. Dropping it stops the server and drops
/// the schema.
pub struct Server {
    pub url: String,
    pub store: Store,
    handle: tokio::runtime::Handle,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    /// `None` when no test database is configured (the test is skipped).
    pub fn start() -> Option<Self> {
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
                let up = (
                    listener.local_addr().unwrap(),
                    db.store.clone(),
                    tokio::runtime::Handle::current(),
                );
                sender.send(Some(up)).unwrap();
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
            Some((addr, store, handle)) => Some(Self {
                url: format!("http://{addr}"),
                store,
                handle,
                stop: Some(stop),
                thread: Some(thread),
            }),
            None => {
                thread.join().unwrap();
                None
            }
        }
    }

    /// How many times agents fetched `memory_id`, read as its owner.
    pub fn fetched(&self, person_id: &str, memory_id: &str) -> u32 {
        self.handle
            .block_on(self.store.get(person_id, memory_id))
            .unwrap()
            .usage
            .fetched
    }

    pub fn distill(&self, person_id: &str, session: &str) {
        assert_eq!(
            self.handle
                .block_on(self.store.distill_session(person_id, session))
                .unwrap()
                .notes,
            2
        );
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

pub fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cairn-agent-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn web(url: &str, token: &str) -> Client {
    Client::new(Config::new(url, token).unwrap())
}

/// The real binary with a clean environment, see [`isolate`].
pub fn command(config_home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cairn"));
    isolate(&mut command, config_home);
    command
}

/// No inherited `CAIRN_*` variable of the developer (their URL, token, config or outbox), and
/// `HOME`, `XDG_CONFIG_HOME` and the other config roots inside `config_home`, which must be
/// under the temporary directory. A test that would reach a path outside it fails here, before
/// anything is spawned.
pub fn isolate(command: &mut Command, config_home: &Path) {
    let temporary = std::env::temp_dir().canonicalize().unwrap();
    // The folder may not exist yet: resolve its nearest existing ancestor.
    let existing = config_home
        .ancestors()
        .find(|dir| dir.exists())
        .and_then(|dir| dir.canonicalize().ok());
    let inside = existing.is_some_and(|home| home.starts_with(&temporary));
    assert!(
        inside,
        "{} is not inside the temporary directory {}",
        config_home.display(),
        temporary.display()
    );
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("CAIRN_") {
            command.env_remove(name);
        }
    }
    command
        .env("HOME", config_home)
        .env("USERPROFILE", config_home)
        .env("APPDATA", config_home)
        .env("XDG_CONFIG_HOME", config_home)
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME")
        .env("CAIRN_NO_UPDATE_CHECK", "1");
}

/// Runs `command` with `input` as its whole stdin.
pub fn with_stdin(command: &mut Command, input: &str) -> Output {
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

/// A git checkout named `repo` under a fresh directory (the repository name is `local:repo`,
/// so two checkouts of the same project agree on it).
pub fn checkout(name: &str) -> PathBuf {
    let dir = temp_dir(name).join("repo");
    std::fs::create_dir_all(&dir).unwrap();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.test"]);
    git(&["config", "user.name", "t"]);
    git(&["commit", "-q", "--allow-empty", "-m", "init"]);
    dir
}

/// One agent: its own config home and device token.
pub struct Agent {
    pub home: PathBuf,
    pub token: String,
}

impl Agent {
    pub fn setup(name: &str, url: &str, token: &str) -> Self {
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

    pub fn run(&self, args: &[&str]) -> Output {
        command(&self.home).args(args).output().unwrap()
    }

    pub fn run_in(&self, dir: &Path, args: &[&str]) -> Output {
        command(&self.home)
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap()
    }

    /// The command's stdout as JSON; it must succeed.
    pub fn json_in(&self, dir: &Path, args: &[&str]) -> Value {
        let output = self.run_in(dir, args);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{args:?}: {stderr}");
        assert!(!stderr.contains(&self.token));
        serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
    }

    pub fn mcp_in(&self, dir: &Path) -> Mcp {
        let mut child = command(&self.home)
            .current_dir(dir)
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

pub struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Mcp {
    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{line}").unwrap();
        let mut response = String::new();
        self.stdout.read_line(&mut response).unwrap();
        serde_json::from_str(&response).unwrap()
    }

    /// The raw `tools/call` result.
    pub fn tool(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))["result"].clone()
    }

    /// A successful call's `content[0].text`, parsed.
    pub fn ok(&mut self, name: &str, arguments: Value) -> Value {
        let result = self.tool(name, arguments);
        assert!(result.get("isError").is_none(), "{name}: {result}");
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn device_token(web: &Client, name: &str) -> String {
    let registered = web
        .post("/cairn-memory/devices/register", &json!({"name": name}))
        .unwrap();
    registered["token"].as_str().unwrap().to_owned()
}

/// An owner and a member, each with a web session, a person id, and a configured agent.
pub struct Team {
    pub owner_web: Client,
    pub owner_id: String,
    pub owner: Agent,
    pub member: Agent,
}

pub fn team(url: &str, label: &str) -> Team {
    let anonymous = web(url, "none");
    let boot = anonymous
        .post(
            "/cairn-memory/auth/bootstrap",
            &json!({"email": "owner@example.test", "password": PASSWORD}),
        )
        .unwrap();
    let owner_web = web(url, boot["token"].as_str().unwrap());
    owner_web
        .post(
            "/cairn-memory/auth/register",
            &json!({"email": "member@example.test", "password": PASSWORD}),
        )
        .unwrap();
    let login = anonymous
        .post(
            "/cairn-memory/auth/login",
            &json!({"email": "member@example.test", "password": PASSWORD}),
        )
        .unwrap();
    let member_web = web(url, login["token"].as_str().unwrap());
    let owner_id = owner_web.get("/cairn-memory/auth/me").unwrap()["person_id"]
        .as_str()
        .unwrap()
        .to_owned();
    Team {
        owner: Agent::setup(
            &format!("{label}-owner"),
            url,
            &device_token(&owner_web, "owner agent"),
        ),
        member: Agent::setup(
            &format!("{label}-member"),
            url,
            &device_token(&member_web, "member agent"),
        ),
        owner_web,
        owner_id,
    }
}

/// `cairn init` in a fresh checkout for the owner, then the same project file in a second
/// checkout that the member joins. Returns (owner dir, member dir, project id).
pub fn shared_project(team: &Team, label: &str) -> (PathBuf, PathBuf, String) {
    let owner_dir = checkout(&format!("{label}-a"));
    let member_dir = checkout(&format!("{label}-b"));
    let init = team.owner.run_in(&owner_dir, &["init", "Cairn Demo"]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let file = std::fs::read_to_string(owner_dir.join(".cairn/project.json")).unwrap();
    std::fs::create_dir_all(member_dir.join(".cairn")).unwrap();
    std::fs::write(member_dir.join(".cairn/project.json"), &file).unwrap();
    let joined = team.member.run_in(&member_dir, &["init"]);
    assert!(
        joined.status.success(),
        "{}",
        String::from_utf8_lossy(&joined.stderr)
    );
    let id = serde_json::from_str::<Value>(&file).unwrap()["project_id"]
        .as_str()
        .unwrap()
        .to_owned();
    (owner_dir, member_dir, id)
}
