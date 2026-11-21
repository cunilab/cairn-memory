use crate::error::ApiResult;
use crate::{AppState, VERSION};
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use serde::Serialize;

#[derive(Serialize)]
pub struct Health {
    ok: bool,
    service: &'static str,
    storage: &'static str,
    semantic_search: &'static str,
    /// True while no account exists; the web console then offers first-account setup.
    bootstrap_required: bool,
    version: &'static str,
    /// The address `cairn setup` is given: `CAIRN_PUBLIC_URL`, else this server's own address.
    public_url: String,
}

pub async fn health(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Health> {
    Ok(Json(Health {
        ok: true,
        service: "cairn-server",
        storage: "postgresql",
        semantic_search: if state.embeddings.is_some() {
            "enabled"
        } else {
            "disabled"
        },
        bootstrap_required: !state.store.has_accounts().await?,
        version: VERSION,
        public_url: state.public_url(&headers),
    }))
}

pub async fn livez() -> Json<serde_json::Value> {
    Json(serde_json::json!({"ok": true, "service": "cairn-server"}))
}
