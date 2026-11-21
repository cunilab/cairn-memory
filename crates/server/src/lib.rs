//! REST transport for Cairn Memory: routes under `/cairn-memory`, on `cairn-store`.

mod auth;
mod error;
pub mod routes;
pub mod worker;

pub use error::ApiError;
pub use worker::LocalEmbeddings;

use axum::Router;
use axum::http::HeaderMap;
use cairn_store::Store;

/// The server's version, for `GET /health` and for judging a device's client.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub embeddings: Option<LocalEmbeddings>,
    /// Lets `/search` with `debug: true` return the gate inputs; only for score calibration.
    pub debug_scores: bool,
    /// `CAIRN_PUBLIC_URL`: the address agents are given, no trailing slash.
    public_url: Option<String>,
}

impl AppState {
    pub fn new(store: Store, embeddings: Option<LocalEmbeddings>) -> Self {
        Self {
            store,
            embeddings,
            debug_scores: false,
            public_url: None,
        }
    }

    pub fn with_public_url(mut self, url: Option<String>) -> Self {
        self.public_url = url
            .map(|u| u.trim().trim_end_matches('/').to_owned())
            .filter(|u| !u.is_empty());
        self
    }

    /// The address to tell agents: `CAIRN_PUBLIC_URL`, else the address this request came to
    /// (a reverse proxy's `x-forwarded-host` and `x-forwarded-proto` win over `host`).
    pub fn public_url(&self, headers: &HeaderMap) -> String {
        if let Some(url) = &self.public_url {
            return url.clone();
        }
        let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
        let host = header("x-forwarded-host")
            .or_else(|| header("host"))
            .map(|h| h.split(',').next().unwrap_or(h).trim())
            .filter(|h| {
                !h.is_empty()
                    && h.chars()
                        .all(|c| c.is_ascii_alphanumeric() || ".:-[]_".contains(c))
            });
        let scheme = match header("x-forwarded-proto").map(str::trim) {
            Some("https") => "https",
            _ => "http",
        };
        format!("{scheme}://{}", host.unwrap_or("127.0.0.1:3111"))
    }
}

/// Every route. Background work is separate: `worker::spawn` in the server, `worker::tick` in tests.
pub fn router(state: AppState) -> Router {
    routes::router().with_state(state)
}
