//! `cairn update` and the daily update notice. Releases come from GitHub Releases; every
//! download is checked against the `SHA256SUMS` asset of the same release. The MCP server and
//! the hooks never call this module, so an agent is never interrupted by an update.

use crate::{
    Error, MAX_RESPONSE_BYTES, config_path, create_private_dir, is_loopback, normalize_url,
    write_private,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use ureq::http::Uri;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const TARGET: &str = env!("CAIRN_TARGET");
const API_URL: &str = "https://api.github.com/repos/cunilab/cairn-memory/releases/latest";
/// Replaces `API_URL` in debug builds only (tests); a release build ignores it, so the
/// environment cannot point an installed `cairn` at another update source.
const API_ENV: &str = "CAIRN_UPDATE_URL";
const NO_CHECK_ENV: &str = "CAIRN_NO_UPDATE_CHECK";
const SUMS_ASSET: &str = "SHA256SUMS";
/// The targets `release.yml` builds; keep the two lists together. Linux uses musl so one
/// static binary runs on any glibc or musl distribution.
const TARGETS: [&str; 4] = [
    "aarch64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-pc-windows-msvc",
];
const API_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const NOTICE_TIMEOUT: Duration = Duration::from_secs(2);
const SELF_CHECK_TIMEOUT: Duration = Duration::from_secs(2);
const NOTICE_INTERVAL_SECS: u64 = 24 * 60 * 60;
const MAX_API_BYTES: u64 = 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

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

fn sha_hex(body: &[u8]) -> String {
    format!("{:x}", Sha256::digest(body))
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
    if expected.eq_ignore_ascii_case(&sha_hex(body)) {
        Ok(())
    } else {
        Err(Error::Request(format!(
            "checksum mismatch for {name}; nothing was installed"
        )))
    }
}

fn api_url() -> Result<String, Error> {
    let custom = std::env::var(API_ENV)
        .ok()
        .filter(|url| cfg!(debug_assertions) && !url.trim().is_empty());
    match custom {
        Some(url) => {
            normalize_url(&url).map_err(|error| Error::Config(format!("{API_ENV}: {error}")))
        }
        None => Ok(API_URL.to_owned()),
    }
}

/// Updates must travel over https. Plain http is allowed to a loopback host in debug builds
/// only, which is how the tests run a local release server.
fn allowed(url: &str) -> bool {
    let Ok(uri) = url.parse::<Uri>() else {
        return false;
    };
    match (uri.scheme_str(), uri.host()) {
        (Some("https"), Some(_)) => true,
        (Some("http"), Some(host)) => cfg!(debug_assertions) && is_loopback(host),
        _ => false,
    }
}

/// Redirects are followed by `fetch`, hop by hop, so each target is checked by `allowed`
/// (GitHub serves assets from another host). The agent sends no credential at all.
fn agent(api: &str, timeout: Duration) -> ureq::Agent {
    let mut builder = ureq::Agent::config_builder()
        .max_redirects(0)
        .max_redirects_will_error(false)
        .http_status_as_error(false)
        .timeout_global(Some(timeout))
        .user_agent(format!("cairn/{VERSION}"))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        );
    // A loopback mirror is plain http; do not route it through an environment proxy.
    if api.starts_with("http://") {
        builder = builder.proxy(None);
    }
    builder.build().into()
}

fn redirect_target(from: &str, location: &str) -> Option<String> {
    if location.starts_with("https://") || location.starts_with("http://") {
        return Some(location.to_owned());
    }
    let uri = from.parse::<Uri>().ok()?;
    if !location.starts_with('/') {
        return None;
    }
    Some(format!(
        "{}://{}{location}",
        uri.scheme_str()?,
        uri.authority()?
    ))
}

fn fetch(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>, Error> {
    let mut url = url.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        if !allowed(&url) {
            return Err(Error::Request(format!("refusing the non-https URL {url}")));
        }
        let mut response = agent
            .get(&url)
            .header(
                "accept",
                "application/vnd.github+json, application/octet-stream",
            )
            .call()
            .map_err(|error| Error::Request(format!("update request failed: {error}")))?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            let location = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .and_then(|location| redirect_target(&url, location));
            url = location.ok_or_else(|| {
                Error::Request(format!(
                    "redirect (HTTP {status}) without a usable location"
                ))
            })?;
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(Error::Request(format!(
                "update request failed: HTTP {status}"
            )));
        }
        return response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|error| Error::Request(format!("cannot read update response: {error}")));
    }
    Err(Error::Request("too many redirects".to_owned()))
}

/// `url` is a GitHub release endpoint: `.../releases/latest` or `.../releases/tags/<tag>`.
fn fetch_release(agent: &ureq::Agent, url: &str) -> Result<Release, Error> {
    let bad = || Error::Request("unexpected release response from GitHub".to_owned());
    let body: Value =
        serde_json::from_slice(&fetch(agent, url, MAX_API_BYTES)?).map_err(|_| bad())?;
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

fn download_sums(agent: &ureq::Agent, release: &Release) -> Result<String, Error> {
    let sums = fetch(agent, release.url(SUMS_ASSET)?, MAX_API_BYTES)?;
    Ok(String::from_utf8_lossy(&sums).into_owned())
}

fn unique_old_path(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".old-{}", now()));
    exe.with_file_name(name)
}

fn old_path(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    exe.with_file_name(name)
}

/// Windows cannot overwrite a running exe but can rename it, so the old file moves aside
/// (to `.old`, or to a unique `.old-<time>` when `.old` is stuck, for `remove_old_exes` to
/// delete on a later run) and the new one takes its name.
fn swap_keeping_old(staged: &Path, exe: &Path) -> io::Result<()> {
    let old = match fs::remove_file(old_path(exe)) {
        Ok(()) => old_path(exe),
        Err(error) if error.kind() == io::ErrorKind::NotFound => old_path(exe),
        Err(_) => unique_old_path(exe),
    };
    fs::rename(exe, &old)?;
    fs::rename(staged, exe).inspect_err(|_| {
        let _ = fs::rename(&old, exe);
    })
}

/// Deletes the previous exes a Windows update left behind.
pub fn remove_old_exe() {
    if cfg!(windows)
        && let Ok(exe) = std::env::current_exe()
    {
        remove_old_exes(&exe);
    }
}

fn remove_old_exes(exe: &Path) {
    let (Some(dir), Some(name)) = (exe.parent(), exe.file_name()) else {
        return;
    };
    let prefix = format!("{}.old", name.to_string_lossy());
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let found = entry.file_name().to_string_lossy().into_owned();
        // `.old` exactly, or `.old-<digits>`; never another file that merely shares the prefix.
        let stale = found == prefix
            || found
                .strip_prefix(&format!("{prefix}-"))
                .is_some_and(|stamp| stamp.bytes().all(|byte| byte.is_ascii_digit()));
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Runs `<exe> --version` for at most `SELF_CHECK_TIMEOUT` and requires `cairn <expected>`,
/// so a binary that cannot run on this machine never replaces the one that does.
fn check_runs(exe: &Path, expected: &str) -> Result<(), Error> {
    let fail = |why: &str| {
        Error::Request(format!(
            "the downloaded binary failed its self-check ({why}); nothing was installed"
        ))
    };
    let mut command = Command::new(exe);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // A just-written file can be briefly busy when another thread forks (ETXTBSY).
    let mut attempts = 0;
    let mut child = loop {
        match command.spawn() {
            Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy && attempts < 10 => {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(fail(&format!("cannot run it: {error}"))),
            Ok(child) => break child,
        }
    };
    let deadline = Instant::now() + SELF_CHECK_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(fail("it did not finish in time"));
            }
        }
    };
    let mut output = String::new();
    if let Some(stdout) = child.stdout.take() {
        let _ = stdout.take(1024).read_to_string(&mut output);
    }
    if status.success() && output.trim() == format!("cairn {expected}") {
        Ok(())
    } else {
        Err(fail("it did not report the expected version"))
    }
}

/// Stages `body` next to `exe` (same filesystem, so the final rename is atomic on unix) with
/// the permission bits of the current exe, runs its self-check, and only then swaps it in.
fn replace_exe(exe: &Path, body: &[u8], expected: &str) -> Result<(), Error> {
    let fail = |error: io::Error| {
        let hint = if error.kind() == io::ErrorKind::PermissionDenied {
            "; re-run `cairn update` with permission to write there"
        } else {
            ""
        };
        Error::Config(format!("cannot replace {}: {error}{hint}", exe.display()))
    };
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".new");
    let staged = exe.with_file_name(name);
    let replaced = (|| {
        write_private(&staged, body).map_err(fail)?;
        fs::set_permissions(&staged, fs::metadata(exe).map_err(fail)?.permissions())
            .map_err(fail)?;
        check_runs(&staged, expected)?;
        if cfg!(windows) {
            swap_keeping_old(&staged, exe)
        } else {
            fs::rename(&staged, exe)
        }
        .map_err(fail)
    })();
    if replaced.is_err() {
        let _ = fs::remove_file(&staged);
    }
    replaced
}

/// Checks (and, unless `check_only`, installs) the latest release; returns what to print.
fn run(api: &str, target: &str, exe: &Path, check_only: bool) -> Result<String, Error> {
    let release = fetch_release(&agent(api, API_TIMEOUT), api)?;
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
    if !newer && release.version != current() {
        return Ok(format!(
            "cairn {VERSION} is newer than the latest release ({}); nothing to update",
            show(release.version)
        ));
    }
    let agent = agent(api, DOWNLOAD_TIMEOUT);
    let sums = download_sums(&agent, &release)?;
    // Download and verify before touching the installed file.
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
    if let Some(binary) = binary {
        replace_exe(exe, &binary, &show(release.version))?;
    }
    Ok(if newer {
        format!("Updated cairn {VERSION} -> {}", show(release.version))
    } else {
        format!("cairn {VERSION} is already the latest release")
    })
}

/// `cairn update [--check]`.
pub fn update(check_only: bool) -> Result<(), Error> {
    let exe = std::env::current_exe()
        .and_then(|exe| exe.canonicalize())
        .map_err(|error| Error::Config(format!("cannot locate the cairn executable: {error}")))?;
    println!("{}", run(&api_url()?, TARGET, &exe, check_only)?);
    if !check_only {
        // The hook commands name the executable; keep them pointing at it.
        for line in crate::setup::refresh(&exe) {
            println!("{line}");
        }
    }
    Ok(())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Returns the newest known version when it is newer than this one. `fetch` runs at most once
/// per `NOTICE_INTERVAL_SECS`. The check is recorded before `fetch` runs, and if the cache
/// cannot be stored (no writable config directory) `fetch` does not run at all: the throttle
/// must hold, or every command would call GitHub. A failed fetch keeps the cached version.
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
        let store = |latest: Option<Version>| {
            let body = json!({ "checkedAt": now, "latest": latest.map(show) });
            let dir = cache
                .parent()
                .ok_or_else(|| io::Error::other("no directory"))?;
            create_private_dir(dir)?;
            write_private(cache, body.to_string().as_bytes())
        };
        if store(latest).is_ok() {
            latest = fetch().or(latest);
            let _ = store(latest);
        }
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
        fetch_release(&agent(&api, NOTICE_TIMEOUT), &api)
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
    use std::io::Write;
    use std::net::TcpListener;

    const LINUX: &str = "x86_64-unknown-linux-musl";

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cairn-update-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Serves `routes` (path -> body) for any number of requests; `routes` gets the base URL
    /// so a release listing can point back at this server. A body that starts with
    /// `REDIRECT ` answers with a 302 to the rest of it.
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
                let _ = if let Some(target) = body.strip_prefix(b"REDIRECT ") {
                    write!(
                        stream,
                        "HTTP/1.1 302 Found\r\nlocation: {}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                        String::from_utf8_lossy(target)
                    )
                } else {
                    write!(
                        stream,
                        "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    )
                    .and_then(|()| stream.write_all(body))
                };
            }
        });
        base
    }

    /// An installed exe stand-in; real ones are executable, and the new file inherits its mode.
    fn old_exe(path: &Path) {
        fs::write(path, "old").unwrap();
        #[cfg(unix)]
        fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }

    /// A stand-in `cairn` binary that reports `version` (unix only: it is a shell script).
    fn fake_binary(version: &str) -> Vec<u8> {
        format!("#!/bin/sh\necho 'cairn {version}'\n").into_bytes()
    }

    /// A mock release `tag` serving `binary`, whose checksum is `binary_sum`.
    fn release(tag: &str, binary: Vec<u8>, binary_sum: String) -> String {
        let name = asset_name(LINUX).unwrap();
        mock(|base| {
            let sums = format!("{binary_sum}  {name}\n");
            let asset = |file: &str| json!({"name": file, "browser_download_url": format!("{base}/dl/{file}")});
            let listing = json!({
                "tag_name": tag,
                "assets": [asset(&name), asset("SHA256SUMS")],
            });
            vec![
                ("/latest".to_owned(), listing.to_string().into_bytes()),
                (format!("/dl/{name}"), binary),
                ("/dl/SHA256SUMS".to_owned(), sums.into_bytes()),
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
            (
                "x86_64-unknown-linux-musl",
                "cairn-x86_64-unknown-linux-musl",
            ),
            (
                "aarch64-unknown-linux-musl",
                "cairn-aarch64-unknown-linux-musl",
            ),
            ("x86_64-pc-windows-msvc", "cairn-x86_64-pc-windows-msvc.exe"),
        ] {
            assert_eq!(asset_name(target).unwrap(), asset);
        }
        for unsupported in ["x86_64-apple-darwin", "x86_64-unknown-linux-gnu"] {
            let error = asset_name(unsupported).unwrap_err();
            assert!(error.to_string().contains("no prebuilt cairn release"));
        }
    }

    #[test]
    fn checksums_must_match_the_named_entry() {
        let sums = format!(
            "{}  cairn-a\n{} *cairn-b\n",
            sha_hex(b"a"),
            sha_hex(b"b").to_uppercase()
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
    fn only_https_urls_are_fetched_and_http_is_loopback_in_tests() {
        assert!(allowed("https://github.com/cunilab/cairn-memory"));
        assert!(allowed("http://127.0.0.1:8080/x") && allowed("http://localhost/x"));
        for bad in [
            "http://example.com/x",
            "http://127.0.0.1.evil.test/x",
            "ftp://x/y",
            "/x",
            "",
        ] {
            assert!(!allowed(bad), "{bad}");
        }
        assert_eq!(
            redirect_target("https://a.test/x", "/y").unwrap(),
            "https://a.test/y"
        );
        assert_eq!(
            redirect_target("https://a.test/x", "https://b.test/z").unwrap(),
            "https://b.test/z"
        );
        assert_eq!(redirect_target("https://a.test/x", "y"), None);
    }

    #[test]
    fn a_redirect_off_loopback_over_http_is_refused() {
        let base = mock(|_| vec![("/a".to_owned(), b"REDIRECT http://example.com/b".to_vec())]);
        let error = fetch(&agent(&base, API_TIMEOUT), &format!("{base}/a"), 1024).unwrap_err();
        assert!(
            error.to_string().contains("refusing the non-https URL"),
            "{error}"
        );
        // A redirect inside the allowed set is followed.
        let base = mock(|base| {
            vec![
                ("/a".to_owned(), format!("REDIRECT {base}/b").into_bytes()),
                ("/b".to_owned(), b"done".to_vec()),
            ]
        });
        assert_eq!(
            fetch(&agent(&base, API_TIMEOUT), &format!("{base}/a"), 1024).unwrap(),
            b"done"
        );
    }

    #[cfg(unix)]
    #[test]
    fn update_installs_a_verified_release() {
        let dir = temp_dir("install");
        let exe = dir.join("cairn");
        fs::write(&exe, "old").unwrap();
        fs::set_permissions(&exe, std::os::unix::fs::PermissionsExt::from_mode(0o750)).unwrap();
        let binary = fake_binary("99.0.0");
        let base = release("v99.0.0", binary.clone(), sha_hex(&binary));
        let api = format!("{base}/latest");

        let report = run(&api, LINUX, &exe, true).unwrap();
        assert!(report.contains("cairn 99.0.0 is available"), "{report}");
        assert_eq!(fs::read(&exe).unwrap(), b"old");

        let report = run(&api, LINUX, &exe, false).unwrap();
        assert!(
            report.starts_with(&format!("Updated cairn {VERSION} -> 99.0.0")),
            "{report}"
        );
        assert_eq!(fs::read(&exe).unwrap(), binary);
        assert!(!dir.join("cairn.new").exists());
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        // The original exe's bits are kept, not forced to 0755.
        assert_eq!(mode(&exe), 0o750);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_binary_that_fails_its_self_check_never_replaces_the_exe() {
        let sleeper = b"#!/bin/sh\nsleep 5\n".to_vec();
        for (binary, why) in [
            (fake_binary("1.2.3"), "did not report the expected version"),
            (b"not an executable".to_vec(), "failed its self-check"),
            (
                b"#!/bin/sh\nexit 1\n".to_vec(),
                "did not report the expected version",
            ),
            (sleeper, "did not finish in time"),
        ] {
            let dir = temp_dir("selfcheck");
            let exe = dir.join("cairn");
            old_exe(&exe);
            let base = release("v99.0.0", binary.clone(), sha_hex(&binary));
            let started = Instant::now();
            let error = run(&format!("{base}/latest"), LINUX, &exe, false).unwrap_err();
            assert!(error.to_string().contains(why), "{error}");
            assert!(started.elapsed() < Duration::from_secs(4));
            assert_eq!(fs::read(&exe).unwrap(), b"old");
            assert!(!dir.join("cairn.new").exists());
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn a_bad_checksum_installs_nothing() {
        let dir = temp_dir("badsum");
        let exe = dir.join("cairn");
        fs::write(&exe, "old").unwrap();
        let base = release("v99.0.0", fake_binary("99.0.0"), sha_hex(b"something else"));
        let error = run(&format!("{base}/latest"), LINUX, &exe, false).unwrap_err();
        assert!(error.to_string().contains("checksum mismatch"), "{error}");
        assert_eq!(fs::read(&exe).unwrap(), b"old");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_latest_release_leaves_the_binary_alone_and_a_newer_build_is_untouched() {
        let dir = temp_dir("latest");
        let exe = dir.join("cairn");
        fs::write(&exe, "old").unwrap();
        let base = release(
            &format!("v{VERSION}"),
            fake_binary(VERSION),
            sha_hex(&fake_binary(VERSION)),
        );
        let api = format!("{base}/latest");
        let report = run(&api, LINUX, &exe, true).unwrap();
        assert_eq!(report, format!("cairn {VERSION} is up to date"));
        let report = run(&api, LINUX, &exe, false).unwrap();
        assert!(report.contains("already the latest release"), "{report}");
        assert_eq!(fs::read(&exe).unwrap(), b"old");

        // A build ahead of the latest release is left alone.
        let base = release(
            "v0.0.1",
            fake_binary("0.0.1"),
            sha_hex(&fake_binary("0.0.1")),
        );
        let report = run(&format!("{base}/latest"), LINUX, &exe, false).unwrap();
        assert!(report.contains("newer than the latest release"), "{report}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreachable_or_odd_release_is_an_error() {
        let dir = temp_dir("errors");
        let exe = dir.join("cairn");
        let missing = mock(|_| vec![]);
        assert!(run(&format!("{missing}/latest"), LINUX, &exe, true).is_err());
        let prerelease = mock(|_| {
            vec![(
                "/latest".to_owned(),
                br#"{"tag_name":"v1.0.0-rc1","assets":[]}"#.to_vec(),
            )]
        });
        let error = run(&format!("{prerelease}/latest"), LINUX, &exe, true).unwrap_err();
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
        old_exe(&exe);
        #[cfg(unix)]
        {
            replace_exe(&exe, &fake_binary("9.9.9"), "9.9.9").unwrap();
            assert_eq!(fs::read(&exe).unwrap(), fake_binary("9.9.9"));
            assert!(!dir.join("cairn.new").exists() && !dir.join("cairn.new.tmp").exists());
        }

        // The Windows path, exercised here: old moves to `.old`, a stale `.old` is replaced.
        let staged = dir.join("cairn.new");
        fs::write(old_path(&exe), "stale").unwrap();
        fs::write(&exe, "current").unwrap();
        fs::write(&staged, "newer").unwrap();
        swap_keeping_old(&staged, &exe).unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"newer");
        assert_eq!(fs::read(old_path(&exe)).unwrap(), b"current");
        // A failed second rename restores the old exe.
        assert!(swap_keeping_old(&dir.join("absent"), &exe).is_err());
        assert_eq!(fs::read(&exe).unwrap(), b"newer");

        // A `.old` that cannot be removed (here a directory) gets a unique name instead,
        // and the next run deletes every leftover but nothing else.
        let _ = fs::remove_file(old_path(&exe));
        fs::create_dir(old_path(&exe)).unwrap();
        fs::write(&staged, "newest").unwrap();
        swap_keeping_old(&staged, &exe).unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"newest");
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("cairn.old-"))
            .collect();
        assert_eq!(leftovers.len(), 1, "{leftovers:?}");
        fs::write(dir.join("cairn.old-notes"), "keep").unwrap();
        remove_old_exes(&exe);
        assert!(!dir.join(&leftovers[0]).exists());
        assert!(dir.join("cairn.old-notes").exists() && old_path(&exe).is_dir());
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
        let result = replace_exe(&exe, &fake_binary("9.9.9"), "9.9.9");
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
        const START: u64 = 10 * NOTICE_INTERVAL_SECS;
        let dir = temp_dir("notice");
        let cache = dir.join("update-check.json");
        let calls = std::cell::Cell::new(0);
        let newer = (99, 0, 0);
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
        // The config directory is created (private) when it is missing.
        let nested = dir.join("fresh").join("update-check.json");
        let before = calls.get();
        assert_eq!(
            daily_check(&nested, START, || fetching(Some(newer))),
            Some(newer)
        );
        assert_eq!((calls.get(), nested.exists()), (before + 1, true));
        // A cache that cannot be stored means no network call, ever.
        let blocked = dir.join("not-a-dir");
        fs::write(&blocked, "file").unwrap();
        let before = calls.get();
        assert_eq!(
            daily_check(&blocked.join("update-check.json"), START, || fetching(
                Some(newer)
            )),
            None
        );
        assert_eq!(calls.get(), before);
        let _ = fs::remove_dir_all(&dir);
    }
}
