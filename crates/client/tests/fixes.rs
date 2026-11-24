mod common;

use common::{Agent, checkout, command, temp_dir, with_stdin};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;

struct Fake {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Fake {
    fn new(action: Value) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let seen = requests.clone();
        let stopped = stop.clone();
        let thread = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut data = Vec::new();
                let mut buf = [0; 4096];
                loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n]);
                    if let Some(end) = data.windows(4).position(|v| v == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&data[..end]);
                        let len = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|s| s.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if data.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                if data.is_empty() {
                    continue;
                }
                let request = String::from_utf8_lossy(&data).to_string();
                let body = if request.starts_with("GET /cairn-memory/device/fixes ") {
                    json!([{ "fix_id":"fx_test", "action":action }]).to_string()
                } else {
                    "{}".to_owned()
                };
                seen.lock().unwrap().push(request);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
            }
        });
        Self {
            url,
            requests,
            stop,
            thread: Some(thread),
        }
    }
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}
impl Drop for Fake {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn applied(fake: &Fake) -> Vec<Value> {
    fake.requests()
        .iter()
        .filter(|r| r.starts_with("POST /cairn-memory/device/fixes/fx_test "))
        .map(|r| serde_json::from_str(r.split_once("\r\n\r\n").unwrap().1).unwrap())
        .collect()
}
#[test]
fn update_guarded() {
    let called = std::cell::Cell::new(false);
    let status = cairn_client::fixes::execute_action(
        json!({"type":"run_update","target":"9999.0.0"}),
        std::path::Path::new("."),
        || {
            called.set(true);
            Ok(())
        },
    );
    assert!(status.is_ok());
    assert!(called.get());
    let fake = Fake::new(json!({"type":"run_update","target":"0.0.1"}));
    let agent = Agent::setup("fix-update", &fake.url, "cdt_test");
    let out = agent.run(&["fixes", "apply"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(applied(&fake)[0]["reason"], "already_current");
    let before = fake.requests().len();
    let _ = with_stdin(
        command(&agent.home).args(["hook", "Stop", "--agent", "claude"]),
        "{}",
    );
    let _ = with_stdin(command(&agent.home).arg("mcp"), "");
    let _ = agent.run(&["search", "probe"]);
    let version = agent.run(&["--version"]);
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        format!("cairn {}", cairn_client::update::VERSION)
    );
    assert!(
        !fake.requests()[before..]
            .iter()
            .any(|r| r.contains("/device/fixes"))
    );
}
#[test]
fn project_file_guards() {
    let checkout = checkout("fix-guards");
    let outside = temp_dir("fix-outside");
    let action = json!({"type":"write_project_file","project_id":"proj_test","repository":"local:repo","name":"Repo"});
    let fake = Fake::new(action.clone());
    let agent = Agent::setup("fix-guards-home", &fake.url, "cdt_test");
    std::fs::create_dir(checkout.join(".cairn")).unwrap();
    std::fs::write(checkout.join(".cairn/project.json"), "keep").unwrap();
    let out = agent.run_in(&checkout, &["fixes", "apply"]);
    assert!(
        out.status.success(),
        "{}; requests: {:?}",
        String::from_utf8_lossy(&out.stderr),
        fake.requests()
    );
    assert_eq!(
        std::fs::read_to_string(checkout.join(".cairn/project.json")).unwrap(),
        "keep"
    );
    assert_eq!(applied(&fake)[0]["reason"], "file_exists");
    std::fs::remove_dir_all(checkout.join(".cairn")).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, checkout.join(".cairn")).unwrap();
        let out = agent.run_in(&checkout, &["fixes", "apply"]);
        assert!(
            out.status.success(),
            "stderr: {}; stdout: {}; requests: {:?}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout),
            fake.requests()
        );
        assert!(!outside.join("project.json").exists());
        assert_eq!(
            applied(&fake).len(),
            2,
            "stderr: {}; requests: {:?}",
            String::from_utf8_lossy(&out.stderr),
            fake.requests()
        );
        assert_eq!(applied(&fake)[1]["reason"], "outside_checkout");
    }
    let other = Fake::new(
        json!({"type":"write_project_file","project_id":"proj_test","repository":"other/repo","name":"Repo"}),
    );
    let second = Agent::setup("fix-other-home", &other.url, "cdt_test");
    let out = second.run_in(&checkout, &["fixes", "apply"]);
    assert!(
        out.status.success(),
        "{}; requests: {:?}",
        String::from_utf8_lossy(&out.stderr),
        other.requests()
    );
    assert_eq!(applied(&other)[0]["reason"], "not_this_repo");
    #[cfg(unix)]
    std::fs::remove_file(checkout.join(".cairn")).unwrap();
    let out = agent.run_in(&checkout, &["fixes", "apply"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let written: Value =
        serde_json::from_slice(&std::fs::read(checkout.join(".cairn/project.json")).unwrap())
            .unwrap();
    assert_eq!(written["project_id"], "proj_test");
    assert_eq!(applied(&fake).last().unwrap()["result"], "applied");
}
#[test]
fn apply_before_command() {
    let fake = Fake::new(json!({"type":"run_update","target":"0.0.1"}));
    let agent = Agent::setup("fix-auto", &fake.url, "cdt_test");
    let out = agent.run(&["doctor"]);
    let requests = fake.requests();
    assert!(
        requests
            .iter()
            .any(|r| r.starts_with("GET /cairn-memory/device/fixes ")),
        "{requests:?}"
    );
    assert_eq!(applied(&fake)[0]["reason"], "already_current");
    assert!(String::from_utf8_lossy(&out.stdout).contains("fx_test"));
}
#[test]
fn closed_actions() {
    for (i, action) in [
        json!({"type":"run_update","target":"0.0.1","extra":true}),
        json!({"type":"arbitrary"}),
    ]
    .into_iter()
    .enumerate()
    {
        let fake = Fake::new(action);
        let agent = Agent::setup(&format!("fix-closed-{i}"), &fake.url, "cdt_test");
        let _ = agent.run(&["fixes", "apply"]);
        assert_eq!(applied(&fake)[0]["reason"], "failed");
    }
}

#[test]
fn setup_refreshes_only_configured_agents() {
    let fake = Fake::new(json!({"type":"suggest_setup","device_id":"dev_test"}));
    let agent = Agent::setup("fix-setup-home", &fake.url, "cdt_test");
    let setup = with_stdin(
        command(&agent.home).args(["setup", &fake.url, "--agent", "claude"]),
        "cdt_test",
    );
    assert!(
        setup.status.success(),
        "{}",
        String::from_utf8_lossy(&setup.stderr)
    );
    let unmanaged = agent.home.join(".codex/hooks.json");
    std::fs::create_dir_all(unmanaged.parent().unwrap()).unwrap();
    std::fs::write(&unmanaged, "{\"other\":true}").unwrap();
    let out = agent.run(&["fixes", "apply"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(applied(&fake)[0]["result"], "applied");
    assert_eq!(
        std::fs::read_to_string(unmanaged).unwrap(),
        "{\"other\":true}"
    );
}
