//! REST transport for Cairn Memory: routes under `/cairn-memory`, on `cairn-store`.

mod auth;
mod error;
pub mod routes;
pub mod worker;

pub use error::ApiError;
pub use worker::LocalEmbeddings;

use auth::AuthRateLimiter;
use axum::Router;
use cairn_store::Store;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub embeddings: Option<LocalEmbeddings>,
    /// Lets `/search` with `debug: true` return the gate inputs; only for score calibration.
    pub debug_scores: bool,
    limiter: Arc<AuthRateLimiter>,
}

impl AppState {
    pub fn new(store: Store, embeddings: Option<LocalEmbeddings>) -> Self {
        Self {
            store,
            embeddings,
            debug_scores: false,
            limiter: Arc::new(AuthRateLimiter::default()),
        }
    }
}

/// Every route. Background work is separate: `worker::spawn` in the server, `worker::tick` in tests.
pub fn router(state: AppState) -> Router {
    routes::router().with_state(state)
}
