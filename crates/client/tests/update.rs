//! The real `cairn` binary: `--version` and `update --check` against a local release listing.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The binary with its own temporary home (see `common::isolate`).
fn cairn() -> Command {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    common::command(&common::temp_dir(&format!(
        "update-{}",
        NEXT.fetch_add(1, Ordering::SeqCst)
    )))
}

/// Answers every request with a release listing for `tag`.
fn release_listing(tag: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/releases/latest", listener.local_addr().unwrap());
    let body = format!(r#"{{"tag_name":"{tag}","assets":[]}}"#);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let _ = stream.read(&mut [0u8; 4096]);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    url
}

#[test]
fn version_flag_and_command_print_the_package_version() {
    for arg in ["--version", "version"] {
        let output = cairn().arg(arg).output().unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("cairn {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
}

#[test]
fn update_check_only_reports() {
    let check = |tag: &str| {
        let output = cairn()
            .args(["update", "--check"])
            .env("CAIRN_UPDATE_URL", release_listing(tag))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    assert!(check("v99.0.0").contains("cairn 99.0.0 is available"));
    assert!(check(&format!("v{}", env!("CARGO_PKG_VERSION"))).contains("is up to date"));
}

/// Serves `routes` (path to body) over plain http on loopback and returns its base URL.
fn serve(routes: Vec<(String, Vec<u8>)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).unwrap_or(0);
            let line = String::from_utf8_lossy(&request[..read]).into_owned();
            let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
            let body = routes.iter().find(|r| r.0 == path).map(|r| r.1.clone());
            let (status, body) = body.map_or(("404 Not Found", Vec::new()), |b| ("200 OK", b));
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(&body);
        }
    });
    base
}

#[test]
#[cfg(unix)]
fn update_points_the_installed_hook_commands_at_the_new_executable() {
    use sha2::{Digest, Sha256};
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("cairn-update-hooks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let setup_env = |command: &mut Command| {
        command
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
            .env("CAIRN_NO_UPDATE_CHECK", "1");
    };
    // Setup from the cargo binary writes hook commands that name it.
    let mut setup = cairn();
    setup_env(&mut setup);
    setup
        .args(["setup", "http://127.0.0.1:9", "--agent", "claude"])
        .stdin(std::process::Stdio::piped());
    let mut child = setup.spawn().unwrap();
    writeln!(child.stdin.take().unwrap(), "test-device").unwrap();
    assert!(child.wait().unwrap().success());

    // The same release feed offers 99.0.0; a copy of the binary updates itself.
    let name = "cairn-x86_64-unknown-linux-musl".to_owned();
    let binary = b"#!/bin/sh\necho 'cairn 99.0.0'\n".to_vec();
    let sums = format!("{:x}  {name}\n", Sha256::digest(&binary));
    let base = serve(vec![
        (format!("/dl/{name}"), binary),
        ("/dl/SHA256SUMS".to_owned(), sums.into_bytes()),
    ]);
    let listing = format!(
        r#"{{"tag_name":"v99.0.0","assets":[{{"name":"{name}","browser_download_url":"{base}/dl/{name}"}},{{"name":"SHA256SUMS","browser_download_url":"{base}/dl/SHA256SUMS"}}]}}"#
    );
    let feed = serve(vec![("/releases/latest".to_owned(), listing.into_bytes())]);
    let copy = dir.join("cairn");
    std::fs::copy(env!("CARGO_BIN_EXE_cairn"), &copy).unwrap();
    let mut update = Command::new(&copy);
    common::isolate(&mut update, &home);
    setup_env(&mut update);
    let output = update
        .arg("update")
        .env("CAIRN_UPDATE_URL", format!("{feed}/releases/latest"))
        .env("CAIRN_UPDATE_TARGET", "x86_64-unknown-linux-musl")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("Updated cairn") && stdout.contains("Cairn claude integration installed"),
        "{stdout}"
    );

    let settings: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join(".claude/settings.json")).unwrap())
            .unwrap();
    let command = settings["hooks"]["Stop"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(
        command.starts_with(&format!("{} hook Stop ", copy.display())),
        "{command}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
