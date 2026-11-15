//! Network client for agents: the `cairn` CLI and the stdio MCP server call the Cairn REST
//! API with a device token. Humans use the web console; this crate never holds an account
//! session and never touches a store file.

pub mod mcp;
pub mod tools;
pub mod update;

use serde_json::{Value, json};
use std::fmt;
use std::fs;
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use ureq::http::Uri;

const URL_ENV: &str = "CAIRN_URL";
const TOKEN_ENV: &str = "CAIRN_DEVICE_TOKEN";
/// Generous for an export bundle, finite so a hostile server cannot fill memory.
const MAX_RESPONSE_BYTES: u64 = 256 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub enum Error {
    /// Missing or invalid setup; fix with `cairn setup`.
    Config(String),
    /// Bad command or tool input, caught before or instead of a request.
    Input(String),
    /// The request did not complete: connection, TLS, timeout, or a refused redirect.
    Request(String),
    /// The server answered with an error status; `message` is the server's own text.
    Status { status: u16, message: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(message) | Self::Input(message) | Self::Request(message) => {
                f.write_str(message)
            }
            Self::Status { status, message } => write!(f, "HTTP {status}: {message}"),
        }
    }
}

impl std::error::Error for Error {}

/// Server URL and device token. `Debug` hides the token.
#[derive(Clone, PartialEq, Eq)]
pub struct Config {
    url: String,
    token: String,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("url", &self.url)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl Config {
    /// Validates the URL: `https`, or `http` for a loopback host only, with no credentials,
    /// query or fragment. A trailing slash is dropped.
    pub fn new(url: &str, token: &str) -> Result<Self, Error> {
        let token = token.trim();
        if token.is_empty() {
            return Err(Error::Config("device token is empty".to_owned()));
        }
        Ok(Self {
            url: normalize_url(url)?,
            token: token.to_owned(),
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// `CAIRN_URL` and `CAIRN_DEVICE_TOKEN` together replace the config file. Either one
    /// alone is an error: it would send the stored token to another server, or another
    /// token to the stored one.
    pub fn load() -> Result<Self, Error> {
        let env = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        };
        Self::load_from(&config_path()?, env(URL_ENV), env(TOKEN_ENV))
    }

    fn load_from(
        path: &Path,
        env_url: Option<String>,
        env_token: Option<String>,
    ) -> Result<Self, Error> {
        match (env_url, env_token) {
            (Some(url), Some(token)) => return Self::new(&url, &token),
            (None, None) => {}
            _ => {
                return Err(Error::Config(format!(
                    "set both {URL_ENV} and {TOKEN_ENV}, or neither"
                )));
            }
        }
        let stored = match fs::read_to_string(path) {
            Ok(raw) => serde_json::from_str::<Value>(&raw)
                .map_err(|_| Error::Config(format!("{} is not valid JSON", path.display())))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
            Err(error) => {
                return Err(Error::Config(format!(
                    "cannot read {}: {error}",
                    path.display()
                )));
            }
        };
        let field = |name: &str| stored.get(name).and_then(Value::as_str);
        let (Some(url), Some(token)) = (field("url"), field("token")) else {
            return Err(Error::Config(
                "not configured: run `cairn setup <server-url>` \
                 or set CAIRN_URL and CAIRN_DEVICE_TOKEN"
                    .to_owned(),
            ));
        };
        Self::new(url, token)
    }

    /// Writes the file with mode 0600 from the start, replacing it atomically. A directory
    /// this call creates gets mode 0700.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        let io = |error: std::io::Error| {
            Error::Config(format!("cannot write {}: {error}", path.display()))
        };
        let dir = path
            .parent()
            .ok_or_else(|| Error::Config("config path has no directory".to_owned()))?;
        create_private_dir(dir).map_err(io)?;
        let body = json!({ "url": self.url, "token": self.token }).to_string();
        write_private(path, body.as_bytes()).map_err(io)
    }
}

/// Creates `dir` and any missing parents with mode 0700; an existing directory keeps its mode.
pub(crate) fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

/// Replaces `path` with `body`, mode 0600 from the first byte. The data goes to a fresh
/// `<path>.tmp` (any existing one, including a symlink, is removed first, and creation fails
/// rather than follow one), then a rename replaces `path` itself, never a symlink's target.
pub fn write_private(path: &Path, body: &[u8]) -> std::io::Result<()> {
    let mut name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("path has no file name"))?
        .to_os_string();
    name.push(".tmp");
    let temporary = path.with_file_name(name);
    match fs::remove_file(&temporary) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&temporary)?;
    let written = file.write_all(body).and_then(|()| file.sync_all());
    drop(file);
    written
        .and_then(|()| fs::rename(&temporary, path))
        .inspect_err(|_| {
            let _ = fs::remove_file(&temporary);
        })
}

/// `$XDG_CONFIG_HOME/cairn/config.json`, else `~/.config/cairn/config.json`, else `%APPDATA%`.
pub fn config_path() -> Result<PathBuf, Error> {
    let env = |name: &str| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let base = env("XDG_CONFIG_HOME")
        .or_else(|| env("HOME").map(|home| home.join(".config")))
        .or_else(|| env("APPDATA"))
        .ok_or_else(|| Error::Config("no user config directory (set HOME)".to_owned()))?;
    Ok(base.join("cairn").join("config.json"))
}

fn normalize_url(url: &str) -> Result<String, Error> {
    let bad = |reason: &str| Error::Config(format!("invalid server URL: {reason}"));
    let uri: Uri = url.trim().parse().map_err(|_| bad("not a URL"))?;
    let authority = uri.authority().ok_or_else(|| bad("no host"))?;
    if authority.as_str().contains('@') {
        return Err(bad("credentials in the URL are not allowed"));
    }
    if uri.query().is_some() || url.contains('#') {
        return Err(bad("query and fragment are not allowed"));
    }
    match uri.scheme_str() {
        Some("https") => {}
        Some("http") if is_loopback(authority.host()) => {}
        Some("http") => {
            return Err(bad(
                "plain http is allowed only for loopback hosts; use https",
            ));
        }
        _ => return Err(bad("scheme must be https (or http on loopback)")),
    }
    let scheme = uri.scheme_str().unwrap_or("https");
    Ok(format!(
        "{scheme}://{authority}{}",
        uri.path().trim_end_matches('/')
    ))
}

fn is_loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// Blocking REST client. It sends only the bearer token it was built with and never
/// follows a redirect, so the token cannot leave the configured origin.
pub struct Client {
    agent: ureq::Agent,
    base: String,
    token: String,
}

impl Client {
    pub fn new(config: Config) -> Self {
        let mut builder = ureq::Agent::config_builder()
            .max_redirects(0)
            .max_redirects_will_error(false)
            .http_status_as_error(false)
            .timeout_global(Some(REQUEST_TIMEOUT))
            // The system trust store, so a server behind a private CA works.
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .build(),
            );
        // A loopback URL is plain http; never hand its token to an HTTP(S)_PROXY from the
        // environment. https through a proxy keeps ureq's default (the environment).
        if config.url.starts_with("http://") {
            builder = builder.proxy(None);
        }
        Self {
            agent: builder.build().into(),
            base: config.url,
            token: config.token,
        }
    }

    pub fn get(&self, path: &str) -> Result<Value, Error> {
        let request = self.agent.get(format!("{}{path}", self.base));
        read(request.header("authorization", self.bearer()).call())
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Value, Error> {
        let request = self.agent.post(format!("{}{path}", self.base));
        read(
            request
                .header("authorization", self.bearer())
                .send_json(body),
        )
    }

    fn bearer(&self) -> String {
        format!("Bearer {}", self.token)
    }
}

fn read(response: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Value, Error> {
    let mut response =
        response.map_err(|error| Error::Request(format!("request failed: {error}")))?;
    let status = response.status().as_u16();
    if (300..400).contains(&status) {
        return Err(Error::Request(format!(
            "server redirected (HTTP {status}); redirects are not followed"
        )));
    }
    let text = response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_string()
        .map_err(|error| Error::Request(format!("cannot read response: {error}")))?;
    if (200..300).contains(&status) {
        return if text.trim().is_empty() {
            Ok(Value::Null)
        } else {
            serde_json::from_str(&text)
                .map_err(|_| Error::Request("server returned invalid JSON".to_owned()))
        };
    }
    // The server's `{"error": ...}` text, or axum's plain-text rejection.
    let message = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| text.trim().chars().take(300).collect());
    Err(Error::Status { status, message })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    const TOKEN: &str = "cdt_secret_value_123";

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cairn-client-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// Serves one canned response and reports the request head it received.
    fn serve_once(response: String) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 4096];
            let read = stream.read(&mut buffer).unwrap();
            // A test that ignores the request head drops the receiver.
            let _ = sender.send(String::from_utf8_lossy(&buffer[..read]).into_owned());
            stream.write_all(response.as_bytes()).unwrap();
        });
        (url, receiver)
    }

    #[test]
    fn setup_writes_a_private_config_that_loads_back() {
        let path = temp_dir("perms").join("cairn").join("config.json");
        Config::new("https://cairn.example.test/", TOKEN)
            .unwrap()
            .save(&path)
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        // Overwriting keeps the mode and leaves no temporary file.
        Config::new("https://cairn.example.test/", TOKEN)
            .unwrap()
            .save(&path)
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(!path.with_extension("json.tmp").exists());
        let loaded = Config::load_from(&path, None, None).unwrap();
        assert_eq!(loaded.url(), "https://cairn.example.test");
        assert_eq!(loaded.token, TOKEN);
        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn environment_replaces_the_file_only_as_a_pair() {
        let path = temp_dir("env").join("config.json");
        Config::new("https://file.example.test", "file-token")
            .unwrap()
            .save(&path)
            .unwrap();
        // One variable alone would aim the stored token at another server (or vice versa).
        for (url, token) in [
            (Some("https://env.example.test".to_owned()), None),
            (None, Some("env-token".to_owned())),
        ] {
            let error = Config::load_from(&path, url, token).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("set both CAIRN_URL and CAIRN_DEVICE_TOKEN"),
                "{error}"
            );
        }
        let loaded = Config::load_from(&path, None, None).unwrap();
        assert_eq!(
            (loaded.url(), loaded.token.as_str()),
            ("https://file.example.test", "file-token")
        );
        let loaded = Config::load_from(
            &path,
            Some("https://env.example.test".into()),
            Some("env-token".into()),
        )
        .unwrap();
        assert_eq!(
            (loaded.url(), loaded.token.as_str()),
            ("https://env.example.test", "env-token")
        );
        let missing = temp_dir("env-none").join("config.json");
        let loaded = Config::load_from(
            &missing,
            Some("https://env.example.test".into()),
            Some("env-token".into()),
        )
        .unwrap();
        assert_eq!(loaded.token, "env-token");
        let error = Config::load_from(&missing, None, None).unwrap_err();
        assert!(error.to_string().contains("cairn setup"));
    }

    #[cfg(unix)]
    #[test]
    fn private_writes_ignore_planted_temp_files_and_never_follow_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let dir = temp_dir("planted");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let temporary = dir.join("config.json.tmp");

        // A stale temporary with loose permissions must not keep them.
        fs::write(&temporary, "stale").unwrap();
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o666)).unwrap();
        write_private(&path, b"one").unwrap();
        assert_eq!(
            (mode(&path), fs::read(&path).unwrap()),
            (0o600, b"one".to_vec())
        );
        assert!(!temporary.exists());

        // A symlink planted at the temporary name is replaced, not written through.
        let victim = dir.join("victim");
        fs::write(&victim, "keep").unwrap();
        symlink(&victim, &temporary).unwrap();
        write_private(&path, b"two").unwrap();
        assert_eq!(fs::read(&victim).unwrap(), b"keep");

        // A symlink at the destination is replaced, and its target is left alone.
        fs::remove_file(&path).unwrap();
        symlink(&victim, &path).unwrap();
        write_private(&path, b"three").unwrap();
        assert_eq!(fs::read(&victim).unwrap(), b"keep");
        assert!(
            !fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            (mode(&path), fs::read(&path).unwrap()),
            (0o600, b"three".to_vec())
        );

        // Created config directories are private; existing ones are untouched.
        let nested = dir.join("a").join("b").join("config.json");
        Config::new("https://cairn.example.test", TOKEN)
            .unwrap()
            .save(&nested)
            .unwrap();
        assert_eq!(mode(&dir.join("a")), 0o700);
        assert_eq!(mode(&dir.join("a").join("b")), 0o700);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn plain_http_is_loopback_only() {
        for ok in [
            "https://cairn.example.test",
            "https://cairn.example.test:8443/base/",
            "http://127.0.0.1:3111",
            "http://localhost:3111",
            "http://[::1]:3111",
        ] {
            assert!(Config::new(ok, TOKEN).is_ok(), "{ok}");
        }
        for bad in [
            "http://cairn.example.test",
            "http://192.168.1.10:3111",
            "http://127.0.0.1.evil.test",
            "http://localhost.evil.test",
            "ftp://127.0.0.1",
            "https://user:pass@cairn.example.test",
            "https://cairn.example.test/?token=1",
            "cairn.example.test",
            "",
        ] {
            assert!(Config::new(bad, TOKEN).is_err(), "{bad}");
        }
        assert!(Config::new("https://cairn.example.test", "  ").is_err());
    }

    #[test]
    fn debug_and_errors_never_show_the_token() {
        let config = Config::new("http://127.0.0.1:1", TOKEN).unwrap();
        assert!(!format!("{config:?}").contains(TOKEN));
        // Nothing listens on port 1, so the request fails; the error must not echo the token.
        let error = Client::new(config)
            .get("/cairn-memory/auth/me")
            .unwrap_err();
        assert!(!error.to_string().contains(TOKEN), "{error}");
    }

    #[test]
    fn a_redirect_is_refused_and_the_other_origin_is_never_contacted() {
        let (other_url, other_request) =
            serve_once("HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}".to_owned());
        let redirect = format!(
            "HTTP/1.1 302 Found\r\nlocation: {other_url}/stolen\r\ncontent-length: 0\r\n\r\n"
        );
        let (url, first_request) = serve_once(redirect);
        let client = Client::new(Config::new(&url, TOKEN).unwrap());
        let error = client
            .post("/cairn-memory/search", &json!({"query": "q"}))
            .unwrap_err();
        assert!(
            error.to_string().contains("redirects are not followed"),
            "{error}"
        );
        assert!(!error.to_string().contains(TOKEN));
        assert!(
            first_request
                .recv()
                .unwrap()
                .contains(&format!("Bearer {TOKEN}"))
        );
        assert!(
            other_request
                .recv_timeout(Duration::from_millis(300))
                .is_err()
        );
    }

    #[test]
    fn server_error_text_is_surfaced_and_no_content_is_null() {
        let body = r#"{"error":"web console session required; device tokens cannot do this"}"#;
        let (url, request) = serve_once(format!(
            "HTTP/1.1 403 Forbidden\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        ));
        let client = Client::new(Config::new(&url, TOKEN).unwrap());
        let error = client.get("/cairn-memory/auth/audit").unwrap_err();
        assert!(matches!(error, Error::Status { status: 403, .. }));
        assert!(error.to_string().contains("device tokens cannot do this"));
        assert!(
            request
                .recv()
                .unwrap()
                .starts_with("GET /cairn-memory/auth/audit ")
        );

        let (url, _) = serve_once("HTTP/1.1 204 No Content\r\n\r\n".to_owned());
        let client = Client::new(Config::new(&url, TOKEN).unwrap());
        assert_eq!(
            client
                .post("/cairn-memory/forget", &json!({"id": "x"}))
                .unwrap(),
            Value::Null
        );
    }
}
