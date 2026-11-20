use crate::AppState;
use crate::error::ApiResult;
use axum::Json;
use axum::extract::State;
use serde::Serialize;

#[derive(Serialize)]
pub struct Health {
    ok: bool,
    service: &'static str,
    storage: &'static str,
    semantic_search: &'static str,
    /// True while no account exists; the web console then offers first-account setup.
    bootstrap_required: bool,
}

pub async fn health(State(state): State<AppState>) -> ApiResult<Health> {
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
    }))
}

pub async fn livez() -> Json<serde_json::Value> {
    Json(serde_json::json!({"ok": true, "service": "cairn-server"}))
}
