use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use cairn_server::{AppState, LocalEmbeddings, app, postgres_app, postgres_app_with_embeddings};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tower::ServiceExt;

async fn request(router: &Router, path: &str, body: Option<Value>, token: Option<&str>) -> Value {
    let mut r = Request::builder()
        .uri(format!("/cairn-memory/{path}"))
        .method(if body.is_some() { "POST" } else { "GET" })
        .header("content-type", "application/json");
    if let Some(t) = token {
        r = r.header("authorization", format!("Bearer {t}"));
    }
    let response = router
        .clone()
        .oneshot(
            r.body(Body::from(body.map(|v| v.to_string()).unwrap_or_default()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(
        status.is_success(),
        "{path}: {status} {}",
        String::from_utf8_lossy(&bytes)
    );
    if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    }
}
fn model() -> LocalEmbeddings {
    let dir = PathBuf::from(
        std::env::var("CAIRN_TEST_EMBEDDING_MODEL_DIR")
            .expect("set CAIRN_TEST_EMBEDDING_MODEL_DIR to the pinned model directory"),
    );
    LocalEmbeddings::load(&dir).unwrap()
}
async fn seed(router: &Router) -> (String, String, String) {
    assert_eq!(
        request(router, "health", None, None).await["semantic_search"],
        "disabled"
    );
    let a = request(
        router,
        "auth/bootstrap",
        Some(json!({"email":"a@semantic.test","password":"correct horse battery staple"})),
        None,
    )
    .await;
    let token = a["token"].as_str().unwrap();
    let other = request(
        router,
        "auth/register",
        Some(json!({"email":"b@semantic.test","password":"correct horse battery staple"})),
        Some(token),
    )
    .await;
    let memory=request(router,"remember",Some(json!({"scope":"personal","title":"Vehicle maintenance","summary":"Service the engine regularly","reason":"Prevent mechanical faults"})),Some(token)).await;
    request(router,"remember",Some(json!({"scope":"personal","title":"Vehicle maintenance","summary":"Service the engine regularly","reason":"Private knowledge"})),Some(other["token"].as_str().unwrap())).await;
    let rows = request(
        router,
        "search",
        Some(json!({"query":"automobile repairs"})),
        Some(token),
    )
    .await;
    assert!(rows.as_array().unwrap().is_empty());
    (
        token.to_owned(),
        other["token"].as_str().unwrap().to_owned(),
        memory["memory_id"].as_str().unwrap().to_owned(),
    )
}
async fn wait_match(router: &Router, token: &str, id: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let hits = request(
            router,
            "search",
            Some(json!({"query":"automobile repairs","limit":100})),
            Some(token),
        )
        .await;
        if hits
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["memory_id"] == id)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "background embedding did not make {id} searchable"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
async fn verify(router: &Router, token: &str, other: &str, id: &str) {
    assert_eq!(
        request(router, "health", None, None).await["semantic_search"],
        "enabled"
    );
    wait_match(router, token, id).await;
    for route in ["search", "smart-search", "context", "recall", "why-changed"] {
        let rows = request(
            router,
            route,
            Some(json!({"query":"automobile repairs","limit":100})),
            Some(token),
        )
        .await;
        let hits = if rows.is_array() {
            rows.as_array().unwrap()
        } else {
            rows["results"].as_array().unwrap()
        };
        assert_eq!(hits.len(), 1, "{route}: {rows}");
        assert_eq!(hits[0]["memory_id"], id);
        assert_eq!(hits[0]["source"], "personal");
        let rows = request(
            router,
            route,
            Some(json!({"query":"automobile repairs","limit":100})),
            Some(other),
        )
        .await;
        let hits = if rows.is_array() {
            rows.as_array().unwrap()
        } else {
            rows["results"].as_array().unwrap()
        };
        assert!(
            hits.iter().all(|r| r["memory_id"] != id),
            "{route}: foreign memory"
        );
    }
    let project = request(
        router,
        "projects/init",
        Some(json!({"repository":"semantic-project","name":"Semantic"})),
        Some(token),
    )
    .await;
    let project_id = project["project_id"].as_str().unwrap();
    let note=request(router,"remember",Some(json!({"scope":"project","context":{"project_id":project_id},"title":"Automotive upkeep","summary":"Inspect the motor","reason":"Safe operation"})),Some(token)).await;
    let note_id = note["memory_id"].as_str().unwrap();
    wait_match(router, token, note_id).await;
    let hits = request(
        router,
        "search",
        Some(json!({"query":"automobile repairs","limit":100})),
        Some(other),
    )
    .await;
    assert!(
        hits.as_array()
            .unwrap()
            .iter()
            .all(|r| r["memory_id"] != note_id)
    );
    let filtered=request(router,"search",Some(json!({"query":"automobile repairs","context":{"project_id":"different-project"},"limit":100})),Some(token)).await;
    assert!(
        filtered
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["memory_id"] != note_id)
    );
    request(
        router,
        "projects/init",
        Some(json!({"repository":"semantic-project","project_id":project_id})),
        Some(other),
    )
    .await;
    let hits = request(
        router,
        "search",
        Some(json!({"query":"automobile repairs","limit":100})),
        Some(other),
    )
    .await;
    assert!(
        hits.as_array()
            .unwrap()
            .iter()
            .any(|r| r["memory_id"] == note_id && r["source"] == "project")
    );
    let next=request(router,"remember",Some(json!({"scope":"global","kind":"rule","title":"Motor servicing","summary":"Replace worn components","reason":"Durable transport"})),Some(token)).await;
    wait_match(router, other, next["memory_id"].as_str().unwrap()).await;
    request(router, "forget", Some(json!({"id":id})), Some(token)).await;
    let hits = request(
        router,
        "search",
        Some(json!({"query":"automobile repairs"})),
        Some(token),
    )
    .await;
    assert!(
        hits.as_array()
            .unwrap()
            .iter()
            .all(|r| r["memory_id"] != id)
    );
}

#[tokio::test]
#[ignore = "requires pinned model in CAIRN_TEST_EMBEDDING_MODEL_DIR"]
async fn file_server_real_local_semantic_backfill_and_writes() {
    let path = std::env::temp_dir().join(format!("cairn-semantic-{}.json", std::process::id()));
    let state = AppState::open(&path).unwrap();
    let off = app(state.clone());
    let (token, other, id) = seed(&off).await;
    drop(off);
    let router = app(state.with_embeddings(model()));
    verify(&router, &token, &other, &id).await;
    std::fs::remove_file(path).unwrap();
}
#[tokio::test]
#[ignore = "requires fresh PostgreSQL and pinned model"]
async fn postgres_server_real_local_semantic_backfill_and_writes() {
    let store =
        cairn_storage::PostgresStore::connect(&std::env::var("CAIRN_TEST_DATABASE_URL").unwrap())
            .await
            .unwrap();
    let (token, other, id) = seed(&postgres_app(store.clone())).await;
    let router = postgres_app_with_embeddings(store, Some(model()));
    verify(&router, &token, &other, &id).await;
}
#[test]
fn missing_or_untrusted_model_fails_closed() {
    assert!(LocalEmbeddings::load(std::path::Path::new("/nonexistent/cairn-minilm")).is_err());
    let dir = std::env::temp_dir().join(format!("cairn-bad-model-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("model.onnx"), b"untrusted model").unwrap();
    assert!(LocalEmbeddings::load(&dir).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}
