//! The real `cairn` binary: `--version` and `update --check` against a local release listing.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;

fn cairn() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cairn"));
    command
        .env_remove("CAIRN_URL")
        .env_remove("CAIRN_DEVICE_TOKEN");
    command
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
