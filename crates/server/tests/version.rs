use std::process::Command;

#[test]
fn server_version_flag_prints_package_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_cairn-server"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("cairn-server {}\n", env!("CARGO_PKG_VERSION"))
    );
}
