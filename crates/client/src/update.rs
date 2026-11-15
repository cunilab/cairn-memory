//! `cairn update` and the daily update notice. Releases come from GitHub Releases; every
//! download is checked against the `SHA256SUMS` asset of the same release. The MCP server and
//! the hooks never call this module, so an agent is never interrupted by an update.

use crate::{
    Error, MAX_RESPONSE_BYTES, config_path, create_private_dir, normalize_url, write_private,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const TARGET: &str = env!("CAIRN_TARGET");
const API_URL: &str = "https://api.github.com/repos/cunilab/cairn-memory/releases/latest";
/// Replaces `API_URL`; `https`, or `http` on loopback (tests and mirrors you run yourself).
const API_ENV: &str = "CAIRN_UPDATE_URL";
const NO_CHECK_ENV: &str = "CAIRN_NO_UPDATE_CHECK";
const SUMS_ASSET: &str = "SHA256SUMS";
const HOOK_ASSETS: [&str; 2] = ["cairn_hook.py", "cairn_outbox.py"];
/// The targets `release.yml` builds; keep the two lists together.
const TARGETS: [&str; 4] = [
    "aarch64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
];
const API_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const NOTICE_TIMEOUT: Duration = Duration::from_secs(2);
const NOTICE_INTERVAL_SECS: u64 = 24 * 60 * 60;
const MAX_API_BYTES: u64 = 1024 * 1024;

type Version = (u64, u64, u64);

struct Release {
    version: Version,
    assets: Vec<(String, String)>,
}

impl Release {
    fn url(&self, name: &str) -> Result<&str, Error> {
        self.assets
            .iter()
            .find(|(asset, _)| asset == name)
            .map(|(_, url)| url.as_str())
            .ok_or_else(|| {
                Error::Request(format!(
                    "release {} has no asset {name}",
                    show(self.version)
                ))
            })
    }
}

/// `1.2.3` or `v1.2.3`; a pre-release or build suffix is not a version we install.
fn parse_version(text: &str) -> Option<Version> {
    let number = |part: &str| {
        part.bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let mut parts = text.strip_prefix('v').unwrap_or(text).split('.');
    let version = (
        number(parts.next()?)?,
        number(parts.next()?)?,
        number(parts.next()?)?,
    );
    parts.next().is_none().then_some(version)
}

fn show((major, minor, patch): Version) -> String {
    format!("{major}.{minor}.{patch}")
}

fn current() -> Version {
    parse_version(VERSION).expect("the workspace version is MAJOR.MINOR.PATCH")
}

/// The raw release asset for the `cairn` binary of `target`.
fn asset_name(target: &str) -> Result<String, Error> {
    if !TARGETS.contains(&target) {
        return Err(Error::Input(format!(
            "no prebuilt cairn release for {target}; build from source"
        )));
    }
    let extension = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    Ok(format!("cairn-{target}{extension}"))
}

/// Checks `body` against its line of a `sha256sum`-style listing (`<hex>  <name>`).
fn verify(sums: &str, name: &str, body: &[u8]) -> Result<(), Error> {
    let expected = sums
        .lines()
        .filter_map(|line| {
            let (hash, file) = line.split_once(char::is_whitespace)?;
            (file.trim_start().trim_start_matches('*') == name).then_some(hash)
        })
        .next()
        .ok_or_else(|| Error::Request(format!("{SUMS_ASSET} has no entry for {name}")))?;
    if expected.eq_ignore_ascii_case(&format!("{:x}", Sha256::digest(body))) {
        Ok(())
    } else {
        Err(Error::Request(format!(
            "checksum mismatch for {name}; nothing was installed"
        )))
    }
}

fn api_url() -> Result<String, Error> {
    match std::env::var(API_ENV)
        .ok()
        .filter(|url| !url.trim().is_empty())
    {
        Some(url) => {
            normalize_url(&url).map_err(|error| Error::Config(format!("{API_ENV}: {error}")))
        }
        None => Ok(API_URL.to_owned()),
    }
}

/// Unlike the REST client this follows redirects (GitHub serves assets from another host),
/// but only over https; it never carries a device token or any credential.
fn agent(api: &str, timeout: Duration) -> ureq::Agent {
    let https_only = !api.starts_with("http://");
    let mut builder = ureq::Agent::config_builder()
        .https_only(https_only)
        .timeout_global(Some(timeout))
        .user_agent(format!("cairn/{VERSION}"))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        );
    // A loopback mirror is plain http; do not route it through an environment proxy.
    if !https_only {
        builder = builder.proxy(None);
    }
    builder.build().into()
}

fn fetch(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>, Error> {
    agent
        .get(url)
        .header(
            "accept",
            "application/vnd.github+json, application/octet-stream",
        )
        .call()
        .map_err(|error| Error::Request(format!("update request failed: {error}")))?
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|error| Error::Request(format!("cannot read update response: {error}")))
}

fn latest_release(agent: &ureq::Agent, api: &str) -> Result<Release, Error> {
    let bad = || Error::Request("unexpected release response from GitHub".to_owned());
    let body: Value =
        serde_json::from_slice(&fetch(agent, api, MAX_API_BYTES)?).map_err(|_| bad())?;
    let tag = body
        .get("tag_name")
        .and_then(Value::as_str)
        .ok_or_else(bad)?;
    let version = parse_version(tag)
        .ok_or_else(|| Error::Request(format!("release tag {tag} is not vMAJOR.MINOR.PATCH")))?;
    let assets = body
        .get("assets")
        .and_then(Value::as_array)
        .ok_or_else(bad)?
        .iter()
        .filter_map(|asset| {
            let field = |name: &str| asset.get(name).and_then(Value::as_str).map(str::to_owned);
            Some((field("name")?, field("browser_download_url")?))
        })
        .collect();
    Ok(Release { version, assets })
}

fn download_verified(
    agent: &ureq::Agent,
    release: &Release,
    sums: &str,
    name: &str,
) -> Result<Vec<u8>, Error> {
    let body = fetch(agent, release.url(name)?, MAX_RESPONSE_BYTES)?;
    verify(sums, name, &body)?;
    Ok(body)
}

fn download_hooks(
    agent: &ureq::Agent,
    release: &Release,
    sums: &str,
) -> Result<Vec<(&'static str, Vec<u8>)>, Error> {
    HOOK_ASSETS
        .iter()
        .map(|name| Ok((*name, download_verified(agent, release, sums, name)?)))
        .collect()
}

fn hook_dir() -> Result<PathBuf, Error> {
    let path = config_path()?;
    Ok(path.with_file_name("hook"))
}

fn write_hooks(dir: &Path, files: &[(&str, Vec<u8>)]) -> Result<(), Error> {
    let io = |error: io::Error| Error::Config(format!("cannot write {}: {error}", dir.display()));
    create_private_dir(dir).map_err(io)?;
    files
        .iter()
        .try_for_each(|(name, body)| write_private(&dir.join(name), body).map_err(io))
}

/// Installs `cairn_hook.py` and `cairn_outbox.py` of the latest release next to the config.
pub fn install_hooks() -> Result<PathBuf, Error> {
    let api = api_url()?;
    let release = latest_release(&agent(&api, API_TIMEOUT), &api)?;
    let agent = agent(&api, DOWNLOAD_TIMEOUT);
    let sums = String::from_utf8_lossy(&fetch(&agent, release.url(SUMS_ASSET)?, MAX_API_BYTES)?)
        .into_owned();
    let dir = hook_dir()?;
    write_hooks(&dir, &download_hooks(&agent, &release, &sums)?)?;
    Ok(dir)
}

/// Windows cannot overwrite a running exe but can rename it, so the old file moves aside
/// (removed by `remove_old_exe` on a later run) and the new one takes its name.
fn swap_keeping_old(staged: &Path, exe: &Path) -> io::Result<()> {
    let old = old_path(exe);
    let _ = fs::remove_file(&old);
    fs::rename(exe, &old)?;
    fs::rename(staged, exe).inspect_err(|_| {
        let _ = fs::rename(&old, exe);
    })
}

fn old_path(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    exe.with_file_name(name)
}

/// Deletes the previous exe a Windows update left behind.
pub fn remove_old_exe() {
    if cfg!(windows)
        && let Ok(exe) = std::env::current_exe()
    {
        let _ = fs::remove_file(old_path(&exe));
    }
}

/// Stages `body` next to `exe` (same filesystem, so the final rename is atomic on unix) and
/// swaps it in. An unwritable directory is reported with the fix.
fn replace_exe(exe: &Path, body: &[u8]) -> Result<(), Error> {
    let fail = |error: io::Error| {
        Error::Config(format!(
            "cannot replace {}: {error}; re-run `cairn update` with permission to write there",
            exe.display()
        ))
    };
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".new");
    let staged = exe.with_file_name(name);
    write_private(&staged, body).map_err(fail)?;
    let swapped = (|| {
        #[cfg(unix)]
        fs::set_permissions(&staged, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
        if cfg!(windows) {
            swap_keeping_old(&staged, exe)
        } else {
            fs::rename(&staged, exe)
        }
    })();
    swapped.map_err(|error| {
        let _ = fs::remove_file(&staged);
        fail(error)
    })
}

/// Checks (and, unless `check_only`, installs) the latest release; returns what to print.
fn run(
    api: &str,
    target: &str,
    exe: &Path,
    hooks: &Path,
    check_only: bool,
) -> Result<String, Error> {
    let release = latest_release(&agent(api, API_TIMEOUT), api)?;
    let newer = release.version > current();
    if check_only {
        return Ok(if newer {
            format!(
                "cairn {} is available (you have {VERSION}); run `cairn update`",
                show(release.version)
            )
        } else {
            format!("cairn {VERSION} is up to date")
        });
    }
    let agent = agent(api, DOWNLOAD_TIMEOUT);
    let sums = String::from_utf8_lossy(&fetch(&agent, release.url(SUMS_ASSET)?, MAX_API_BYTES)?)
        .into_owned();
    // Download and verify everything before touching the installed files.
    let binary = if newer {
        Some(download_verified(
            &agent,
            &release,
            &sums,
            &asset_name(target)?,
        )?)
    } else {
        None
    };
    let scripts = download_hooks(&agent, &release, &sums)?;
    if let Some(binary) = binary {
        replace_exe(exe, &binary)?;
    }
    write_hooks(hooks, &scripts)?;
    Ok(if newer {
        format!(
            "Updated cairn {VERSION} -> {}; hook scripts are in {}",
            show(release.version),
            hooks.display()
        )
    } else {
        format!(
            "cairn {VERSION} is already the latest release; hook scripts refreshed in {}",
            hooks.display()
        )
    })
}

/// `cairn update [--check]`.
pub fn update(check_only: bool) -> Result<(), Error> {
    let exe = std::env::current_exe()
        .and_then(|exe| exe.canonicalize())
        .map_err(|error| Error::Config(format!("cannot locate the cairn executable: {error}")))?;
    println!(
        "{}",
        run(&api_url()?, TARGET, &exe, &hook_dir()?, check_only)?
    );
    Ok(())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Returns the newest known version when it is newer than this one. `fetch` runs at most once
/// per `NOTICE_INTERVAL_SECS`; a failed fetch keeps the cached version and still counts as a
/// check, so an offline machine does not pay the timeout on every command.
fn daily_check(cache: &Path, now: u64, fetch: impl FnOnce() -> Option<Version>) -> Option<Version> {
    let stored: Value = fs::read_to_string(cache)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or(Value::Null);
    let checked_at = stored.get("checkedAt").and_then(Value::as_u64).unwrap_or(0);
    let mut latest = stored
        .get("latest")
        .and_then(Value::as_str)
        .and_then(parse_version);
    if now.saturating_sub(checked_at) >= NOTICE_INTERVAL_SECS {
        latest = fetch().or(latest);
        let body = json!({ "checkedAt": now, "latest": latest.map(show) });
        let _ = write_private(cache, body.to_string().as_bytes());
    }
    latest.filter(|latest| *latest > current())
}

/// One stderr line when a newer release is known. Silent when stderr is not a terminal,
/// when `CAIRN_NO_UPDATE_CHECK=1`, or on any failure.
pub fn notify() {
    if std::env::var(NO_CHECK_ENV).is_ok_and(|value| value == "1") || !io::stderr().is_terminal() {
        return;
    }
    let Ok(config) = config_path() else { return };
    let fetch = || {
        let api = api_url().ok()?;
        latest_release(&agent(&api, NOTICE_TIMEOUT), &api)
            .ok()
            .map(|release| release.version)
    };
    if let Some(latest) = daily_check(&config.with_file_name("update-check.json"), now(), fetch) {
        eprintln!(
            "cairn {} is available (you have {VERSION}) \u{2014} run `cairn update`",
            show(latest)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    const LINUX: &str = "x86_64-unknown-linux-gnu";

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cairn-update-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sha(body: &[u8]) -> String {
        format!("{:x}", Sha256::digest(body))
    }

    /// Serves `routes` (path -> body) for any number of requests; `routes` gets the base URL
    /// so a release listing can point back at this server.
    fn mock(routes: impl FnOnce(&str) -> Vec<(String, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes = routes(&base);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head);
                let path = head.split_whitespace().nth(1).unwrap_or_default();
                let (status, body) = match routes.iter().find(|(route, _)| route == path) {
                    Some((_, body)) => ("200 OK", body.as_slice()),
                    None => ("404 Not Found", b"missing".as_slice()),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(body);
            }
        });
        base
    }

    const BINARY: &[u8] = b"new cairn binary";
    const HOOK: &[u8] = b"print('hook')";
    const OUTBOX: &[u8] = b"print('outbox')";

    /// A mock release `tag` whose checksum for the binary is `binary_sum`.
    fn release(tag: &str, binary_sum: String) -> String {
        let name = asset_name(LINUX).unwrap();
        mock(|base| {
            let sums = format!(
                "{binary_sum}  {name}\n{}  cairn_hook.py\n{}  cairn_outbox.py\n",
                sha(HOOK),
                sha(OUTBOX)
            );
            let asset = |file: &str| json!({"name": file, "browser_download_url": format!("{base}/dl/{file}")});
            let listing = json!({
                "tag_name": tag,
                "assets": [asset(&name), asset("SHA256SUMS"), asset("cairn_hook.py"), asset("cairn_outbox.py")],
            });
            vec![
                ("/latest".to_owned(), listing.to_string().into_bytes()),
                (format!("/dl/{name}"), BINARY.to_vec()),
                ("/dl/SHA256SUMS".to_owned(), sums.into_bytes()),
                ("/dl/cairn_hook.py".to_owned(), HOOK.to_vec()),
                ("/dl/cairn_outbox.py".to_owned(), OUTBOX.to_vec()),
            ]
        })
    }

    #[test]
    fn versions_parse_strictly_and_compare_numerically() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.10.0"), Some((0, 10, 0)));
        for bad in [
            "",
            "1.2",
            "1.2.3.4",
            "1.2.x",
            "v1.2.3-rc1",
            "1.2.+3",
            "1..3",
            "vv1.2.3",
        ] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
        assert!(parse_version("1.0.0") > parse_version("0.99.99"));
        assert!(parse_version("1.2.3") == parse_version("v1.2.3"));
        assert_eq!(parse_version(VERSION), Some(current()));
    }

    #[test]
    fn assets_are_chosen_per_target() {
        for (target, asset) in [
            ("aarch64-apple-darwin", "cairn-aarch64-apple-darwin"),
            ("x86_64-unknown-linux-gnu", "cairn-x86_64-unknown-linux-gnu"),
            (
                "aarch64-unknown-linux-gnu",
                "cairn-aarch64-unknown-linux-gnu",
            ),
            ("x86_64-pc-windows-msvc", "cairn-x86_64-pc-windows-msvc.exe"),
        ] {
            assert_eq!(asset_name(target).unwrap(), asset);
        }
        let error = asset_name("x86_64-apple-darwin").unwrap_err();
        assert!(error.to_string().contains("no prebuilt cairn release"));
    }

    #[test]
    fn checksums_must_match_the_named_entry() {
        let sums = format!(
            "{}  cairn-a\n{} *cairn-b\n",
            sha(b"a"),
            sha(b"b").to_uppercase()
        );
        verify(&sums, "cairn-a", b"a").unwrap();
        verify(&sums, "cairn-b", b"b").unwrap();
        assert!(
            verify(&sums, "cairn-a", b"b")
                .unwrap_err()
                .to_string()
                .contains("mismatch")
        );
        assert!(
            verify(&sums, "cairn-c", b"a")
                .unwrap_err()
                .to_string()
                .contains("no entry")
        );
        assert!(verify("", "cairn-a", b"a").is_err());
    }

    #[test]
    fn update_installs_a_verified_release_and_refreshes_the_hooks() {
        let dir = temp_dir("install");
        let exe = dir.join("cairn");
        fs::write(&exe, "old").unwrap();
        let hooks = dir.join("config").join("hook");
        let base = release("v99.0.0", sha(BINARY));
        let api = format!("{base}/latest");

        let report = run(&api, LINUX, &exe, &hooks, true).unwrap();
        assert!(report.contains("cairn 99.0.0 is available"), "{report}");
        assert_eq!(fs::read(&exe).unwrap(), b"old");
        assert!(!hooks.exists());

        let report = run(&api, LINUX, &exe, &hooks, false).unwrap();
        assert!(
            report.starts_with(&format!("Updated cairn {VERSION} -> 99.0.0")),
            "{report}"
        );
        assert_eq!(fs::read(&exe).unwrap(), BINARY);
        assert_eq!(fs::read(hooks.join("cairn_hook.py")).unwrap(), HOOK);
        assert_eq!(fs::read(hooks.join("cairn_outbox.py")).unwrap(), OUTBOX);
        assert!(!dir.join("cairn.new").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&exe), 0o755);
            assert_eq!(mode(&hooks), 0o700);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_checksum_installs_nothing() {
        let dir = temp_dir("badsum");
        let exe = dir.join("cairn");
        fs::write(&exe, "old").unwrap();
        let hooks = dir.join("hook");
        let base = release("v99.0.0", sha(b"something else"));
        let error = run(&format!("{base}/latest"), LINUX, &exe, &hooks, false).unwrap_err();
        assert!(error.to_string().contains("checksum mismatch"), "{error}");
        assert_eq!(fs::read(&exe).unwrap(), b"old");
        assert!(!hooks.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_latest_release_leaves_the_binary_alone() {
        let dir = temp_dir("latest");
        let exe = dir.join("cairn");
        fs::write(&exe, "old").unwrap();
        let hooks = dir.join("hook");
        let base = release(&format!("v{VERSION}"), sha(BINARY));
        let api = format!("{base}/latest");
        let report = run(&api, LINUX, &exe, &hooks, true).unwrap();
        assert_eq!(report, format!("cairn {VERSION} is up to date"));
        let report = run(&api, LINUX, &exe, &hooks, false).unwrap();
        assert!(report.contains("already the latest release"), "{report}");
        assert_eq!(fs::read(&exe).unwrap(), b"old");
        assert_eq!(fs::read(hooks.join("cairn_hook.py")).unwrap(), HOOK);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreachable_or_odd_release_is_an_error() {
        let dir = temp_dir("errors");
        let exe = dir.join("cairn");
        let hooks = dir.join("hook");
        let missing = mock(|_| vec![]);
        assert!(run(&format!("{missing}/latest"), LINUX, &exe, &hooks, true).is_err());
        let prerelease = mock(|_| {
            vec![(
                "/latest".to_owned(),
                br#"{"tag_name":"v1.0.0-rc1","assets":[]}"#.to_vec(),
            )]
        });
        let error = run(&format!("{prerelease}/latest"), LINUX, &exe, &hooks, true).unwrap_err();
        assert!(
            error.to_string().contains("not vMAJOR.MINOR.PATCH"),
            "{error}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replacing_the_exe_is_atomic_and_both_swaps_work() {
        let dir = temp_dir("replace");
        let exe = dir.join("cairn");
        fs::write(&exe, "old").unwrap();
        replace_exe(&exe, b"new").unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"new");
        assert!(!dir.join("cairn.new").exists() && !dir.join("cairn.new.tmp").exists());

        // The Windows path, exercised here: old moves to `.old`, a stale `.old` is replaced.
        let staged = dir.join("cairn.new");
        fs::write(old_path(&exe), "stale").unwrap();
        fs::write(&staged, "newer").unwrap();
        swap_keeping_old(&staged, &exe).unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"newer");
        assert_eq!(fs::read(old_path(&exe)).unwrap(), b"new");
        // A failed second rename restores the old exe.
        assert!(swap_keeping_old(&dir.join("absent"), &exe).is_err());
        assert_eq!(fs::read(&exe).unwrap(), b"newer");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn an_unwritable_directory_says_how_to_fix_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("readonly");
        let exe = dir.join("cairn");
        fs::write(&exe, "old").unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        let probe = fs::write(dir.join("probe"), "").is_ok(); // running as root ignores modes
        let result = replace_exe(&exe, b"new");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        if !probe {
            let error = result.unwrap_err().to_string();
            assert!(error.contains("with permission to write there"), "{error}");
            assert_eq!(fs::read(&exe).unwrap(), b"old");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_notice_checks_at_most_once_a_day_and_fails_silently() {
        let dir = temp_dir("notice");
        let cache = dir.join("update-check.json");
        let calls = std::cell::Cell::new(0);
        let newer = (99, 0, 0);
        const START: u64 = 10 * NOTICE_INTERVAL_SECS;
        let fetching = |latest: Option<Version>| {
            calls.set(calls.get() + 1);
            latest
        };

        assert_eq!(
            daily_check(&cache, START, || fetching(Some(newer))),
            Some(newer)
        );
        assert_eq!(calls.get(), 1);
        // Within a day: cached answer, no fetch.
        assert_eq!(
            daily_check(&cache, START + NOTICE_INTERVAL_SECS - 1, || fetching(None)),
            Some(newer)
        );
        assert_eq!(calls.get(), 1);
        // A failed fetch after a day keeps the cached version and restarts the interval.
        let later = START + NOTICE_INTERVAL_SECS;
        assert_eq!(daily_check(&cache, later, || fetching(None)), Some(newer));
        assert_eq!(calls.get(), 2);
        assert_eq!(
            daily_check(&cache, later + 1, || fetching(None)),
            Some(newer)
        );
        assert_eq!(calls.get(), 2);
        // Nothing newer than this build: no notice.
        let cache = dir.join("other.json");
        assert_eq!(
            daily_check(&cache, START, || fetching(Some(current()))),
            None
        );
        // An unreadable cache is a stale one; the cache holds no token or other secret.
        fs::write(&cache, "not json").unwrap();
        assert_eq!(
            daily_check(&cache, START, || fetching(Some(newer))),
            Some(newer)
        );
        assert!(!fs::read_to_string(&cache).unwrap().contains("token"));
        let _ = fs::remove_dir_all(&dir);
    }
}
