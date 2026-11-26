//! Runs the real `cairn` binary (CLI and `cairn mcp`) against an in-process Cairn server on a
//! `TestDb` schema: the five memory tools through both transports, plus config and argument handling.

mod common;

use common::*;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;

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
        .args(["search", "probe"])
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
        .args(["search", "probe"])
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
            .args(["search", "probe"])
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
        run.env("CAIRN_CONFIG", &picked).args(["search", "probe"]);
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

    let default_only = command(&home).args(["search", "probe"]).output().unwrap();
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
    let output = command(&home).args(["search", "probe"]).output().unwrap();
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
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memory_search","arguments":{"query":"probe"}}}"#,
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
    let output = run.args(["search", "probe"]).output().unwrap();
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
        "init", "setup", "mcp", "update", "version", "doctor", "search", "get", "save", "feedback",
        "forget",
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
        vec!["search", "q", "--bogus"],
        vec!["get", "--bogus"],
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
    assert!(String::from_utf8_lossy(&output.stderr).contains("web console"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

const DECISION: &str = r#"{"kind":"decision","title":"Use Postgres SKIP LOCKED for the job queue","summary":"Background jobs run from a Postgres table claimed with SKIP LOCKED.","reason":"Workers never block each other and there is one datastore.","fields":{"context":"job queue","alternatives":["Redis"],"consequences":"one datastore"}}"#;

#[test]
fn the_five_tools_work_through_the_cli_and_mcp() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "flow");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let config = team.owner.home.join("cairn").join("config.json");
        assert_eq!(
            std::fs::metadata(config).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let (dir_a, dir_b, _project) = shared_project(&team, "flow");
    let (owner, member) = (&team.owner, &team.member);

    // save: argument, then stdin; the same record is deduplicated.
    let saved = owner.json_in(&dir_a, &["save", DECISION]);
    assert_eq!(saved["status"], "active");
    assert_eq!(saved["deduplicated"], false);
    let id = saved["id"].as_str().unwrap().to_owned();
    let mut piped = command(&owner.home);
    let again = with_stdin(piped.current_dir(&dir_a).args(["save", "-"]), DECISION);
    assert!(again.status.success());
    let again: Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(
        (again["id"].as_str(), again["deduplicated"].as_bool()),
        (Some(id.as_str()), Some(true))
    );

    // search: cards for the owner and for a project member; the kind filter applies.
    let found = owner.json_in(
        &dir_a,
        &[
            "search", "skip", "locked", "--kind", "decision", "--limit", "3",
        ],
    );
    assert_eq!(found["results"][0]["id"], id.as_str());
    // An absolute score, not a share of the top card's.
    assert!(
        found["results"][0]["score"]
            .as_f64()
            .is_some_and(|s| s > 0.5 && s <= 1.0)
    );
    let theirs = member.json_in(&dir_b, &["search", "skip locked"]);
    assert_eq!(theirs["results"][0]["id"], id.as_str());
    let none = owner.json_in(&dir_a, &["search", "skip locked", "--kind", "rule"]);
    assert_eq!(none["results"], json!([]));

    // get: full record plus the unknown id under `missing`.
    let got = member.json_in(&dir_b, &["get", &id, "mem_unknown"]);
    assert_eq!(got["memories"][0]["id"], id.as_str());
    assert_eq!(
        got["memories"][0]["fields"]["alternatives"],
        json!(["Redis"])
    );
    assert_eq!(got["missing"], json!(["mem_unknown"]));

    // feedback and forget: only the owner may forget.
    let voted = member.json_in(&dir_b, &["feedback", &id, "helpful"]);
    assert_eq!(voted, json!({"id": id, "status": "active"}));
    assert!(!member.run_in(&dir_b, &["forget", &id]).status.success());

    // MCP lists the memory and skill tools, with instructions sent once at initialize.
    let mut mcp = owner.mcp_in(&dir_a);
    let listed = mcp.request("tools/list", json!({}));
    let names: Vec<_> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "memory_search",
            "memory_get",
            "memory_save",
            "memory_feedback",
            "memory_forget",
            "skill_search",
            "skill_get",
            "skill_save",
            "skill_feedback"
        ]
    );
    let via_mcp = mcp.ok("memory_search", json!({"query": "skip locked"}));
    assert_eq!(via_mcp["results"][0]["id"], id.as_str());
    let forgotten = mcp.ok("memory_forget", json!({"id": id}));
    assert_eq!(forgotten, json!({"id": id, "status": "forgotten"}));
    let after = mcp.ok("memory_get", json!({"ids": [id]}));
    assert_eq!(after["missing"], json!([id]));
}

#[test]
fn non_git_project_saves_attach_from_root_and_child_for_cli_and_mcp() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "plain-save");
    let root = temp_dir("plain-save-root");
    let child = root.join("nested");
    std::fs::create_dir(&child).unwrap();
    assert!(team.owner.run_in(&root, &["init"]).status.success());
    let identity: Value =
        serde_json::from_slice(&std::fs::read(root.join(".cairn/project.json")).unwrap()).unwrap();
    for dir in [&root, &child] {
        let mut input: Value = serde_json::from_str(DECISION).unwrap();
        input["scope"] = json!("project");
        let cli = team.owner.json_in(dir, &["save", &input.to_string()]);
        let mut mcp = team.owner.mcp_in(dir);
        let saved = mcp.ok("memory_save", input);
        assert_eq!(saved["id"], cli["id"]);
        let full = mcp.ok("memory_get", json!({"ids": [saved["id"]]}));
        assert_eq!(full["memories"][0]["scope"], "project");
        let view = team
            .owner_web
            .get(&format!(
                "/cairn-memory/memories/{}",
                saved["id"].as_str().unwrap()
            ))
            .unwrap();
        assert_eq!(view["project_id"], identity["project_id"]);
    }
}

#[test]
fn init_keeps_same_named_no_remote_folders_separate() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "local-collision");
    let first = checkout("local-collision-first");
    let second = checkout("local-collision-second");
    let identity = |dir: &std::path::Path| -> Value {
        serde_json::from_slice(&std::fs::read(dir.join(".cairn/project.json")).unwrap()).unwrap()
    };
    for dir in [&first, &second] {
        let output = team.owner.run_in(dir, &["init"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let first_file = identity(&first);
    let second_file = identity(&second);
    assert_eq!(first_file["repository"], "local:repo");
    assert_eq!(second_file["repository"], "local:repo");
    assert_ne!(first_file["project_id"], second_file["project_id"]);
    for (dir, original) in [(&first, &first_file), (&second, &second_file)] {
        let output = team.owner.run_in(dir, &["init"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(&identity(dir), original);
    }
}

#[test]
fn init_registers_a_committed_project_again_after_a_server_wipe() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "wipe");
    let dir = checkout("wipe");
    std::fs::create_dir_all(dir.join(".cairn")).unwrap();
    let id = "proj_zzzzzzzzzzzzzzzzzzzzzzzzzz";
    std::fs::write(
        dir.join(".cairn/project.json"),
        format!(r#"{{"project_id":"{id}","repository":"local:wipe","name":"Wiped"}}"#),
    )
    .unwrap();
    // The file is fine but the server has never heard of the id: doctor says so.
    let before = team.owner.run_in(&dir, &["doctor"]);
    let text = String::from_utf8_lossy(&before.stdout).into_owned();
    assert_eq!(before.status.code(), Some(1), "{text}");
    assert!(
        text.contains(&format!("fail  project  {id} is not on the server")),
        "{text}"
    );
    // A plain init refuses: whoever registers an unknown id first owns it. Nothing is written.
    let refused = team.owner.run_in(&dir, &["init"]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!refused.status.success(), "{said}");
    assert!(said.contains("cairn init --yes"), "{said}");
    let still = team.owner.run_in(&dir, &["doctor"]);
    assert!(
        String::from_utf8_lossy(&still.stdout).contains("is not on the server"),
        "still unregistered"
    );
    // With --yes it registers the same id again, as owner, and warns why.
    let init = team.owner.run_in(&dir, &["init", "--yes"]);
    let said = String::from_utf8_lossy(&init.stdout).into_owned();
    assert!(
        init.status.success(),
        "{said}{}",
        String::from_utf8_lossy(&init.stderr)
    );
    assert!(
        said.contains(&format!("Warning: the server did not know project {id}")),
        "{said}"
    );
    assert!(
        std::fs::read_to_string(dir.join(".cairn/project.json"))
            .unwrap()
            .contains(id)
    );
    let after = team.owner.run_in(&dir, &["doctor"]);
    let text = String::from_utf8_lossy(&after.stdout).into_owned();
    assert!(text.contains(&format!("ok    project  {id}")), "{text}");
}

#[test]
fn doctor_reports_each_check_and_fails_when_the_server_is_unreachable() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "doctor");
    let (dir, _, _) = shared_project(&team, "doctor");
    let run = |agent: &Agent, dir: &std::path::Path| {
        command(&agent.home)
            .env("HOME", &agent.home)
            .current_dir(dir)
            .arg("doctor")
            .output()
            .unwrap()
    };
    let output = run(&team.owner, &dir);
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(output.status.success(), "{text}");
    for line in [
        "ok    config",
        "ok    server",
        "warn  semantic",
        "ok    device",
        "ok    project",
    ] {
        assert!(text.contains(line), "{line}: {text}");
    }
    assert!(!text.contains(&team.owner.token), "{text}");

    // Inside git without .cairn/project.json: project memories are hidden, so say how to fix it.
    let bare = temp_dir("doctor-bare");
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&bare)
            .status()
            .unwrap()
            .success()
    );
    let text = String::from_utf8_lossy(&run(&team.owner, &bare).stdout).into_owned();
    let line = text.lines().find(|l| l.contains("project")).unwrap_or("");
    assert!(line.starts_with("warn  project"), "{text}");
    assert!(line.contains("run cairn init"), "{text}");

    // Same token, nothing listening: the server and device checks cannot pass.
    let dead = Agent::setup("doctor-dead", "http://127.0.0.1:1", "cdt_dead_server");
    let output = run(&dead, &dir);
    let text = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("fail  server"), "{text}");

    // A live server that rejects the token: the device line says where to fix it.
    let rejected = Agent::setup("doctor-401", &server.url, "cdt_not_a_real_device_token");
    let output = run(&rejected, &dir);
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(output.status.code(), Some(1), "{text}");
    let device = text.lines().find(|l| l.contains("device")).unwrap_or("");
    assert!(device.starts_with("fail  device"), "{text}");
    assert!(device.contains("401"), "{text}");
    assert!(device.contains("config.json"), "{text}");
    assert!(device.contains("run cairn setup"), "{text}");
    assert!(!text.contains("cdt_not_a_real_device_token"), "{text}");
}
