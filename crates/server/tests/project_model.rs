use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
};
use cairn_server::{AppState, app, postgres_app};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn request(
    router: &Router,
    method: &str,
    path: &str,
    body: Value,
    token: Option<&str>,
) -> (u16, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(format!("/cairn-memory/{path}"))
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = router
        .clone()
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}
async fn ok(router: &Router, method: &str, path: &str, body: Value, token: Option<&str>) -> Value {
    let (status, body) = request(router, method, path, body, token).await;
    assert!(
        (200..300).contains(&status),
        "{method} {path}: {status} {body}"
    );
    body
}
async fn scenario(router: Router) {
    let pw = "correct horse battery staple";
    let admin = ok(
        &router,
        "POST",
        "auth/bootstrap",
        json!({"email":"admin@scope.test","password":pw}),
        None,
    )
    .await;
    let admin_token = admin["token"].as_str().unwrap();
    let author = ok(
        &router,
        "POST",
        "auth/register",
        json!({"email":"author@scope.test","password":pw}),
        Some(admin_token),
    )
    .await;
    let stranger = ok(
        &router,
        "POST",
        "auth/register",
        json!({"email":"stranger@scope.test","password":pw}),
        Some(admin_token),
    )
    .await;
    let author_token = author["token"].as_str().unwrap();
    let stranger_token = stranger["token"].as_str().unwrap();
    let project = ok(
        &router,
        "POST",
        "projects/init",
        json!({"repository":"example.test/acme/scopes","name":"Scopes"}),
        Some(admin_token),
    )
    .await;
    let project_id = project["project_id"].as_str().unwrap();
    ok(
        &router,
        "POST",
        "projects/init",
        json!({"repository":"example.test/acme/scopes","project_id":project_id}),
        Some(author_token),
    )
    .await;
    let context =
        json!({"project_id":project_id,"repository":"example.test/acme/scopes","branch":"feature"});
    for missing in ["title", "summary", "reason", "scope"] {
        let mut input = json!({"title":"scopecheck title","summary":"scopecheck summary","reason":"scopecheck reason","scope":"personal"});
        input.as_object_mut().unwrap().remove(missing);
        let (status, body) = request(&router, "POST", "remember", input, Some(author_token)).await;
        assert_eq!(status, 400, "{missing}: {body}");
        assert!(body["error"].as_str().unwrap().contains(missing), "{body}");
    }
    let note = ok(&router,"POST","remember",json!({"scope":"project","kind":"decision","title":"scopecheck decision","summary":"Choose bounded writes","reason":"Avoid races","context":context,"decision_context":"Concurrent capture","alternatives":["unbounded"],"consequences":"small transactions","details":"verified details"}),Some(author_token)).await;
    let personal = ok(&router,"POST","remember",json!({"scope":"personal","kind":"preference","title":"scopecheck behaviour","summary":"Use concise answers","reason":"Save attention","preference":"concise","example":"one paragraph"}),Some(author_token)).await;
    let global = ok(&router,"POST","remember",json!({"scope":"global","kind":"gotcha","title":"scopecheck global","summary":"Reuse locks","reason":"Serialize writes","symptom":"race","cause":"missing lock","fix":"transaction"}),Some(author_token)).await;
    let device = ok(&router,"POST","devices/register",json!({"deviceId":"scope-device","personId":author["account"]["person_id"],"name":"Scope device"}),Some(author_token)).await;
    ok(
        &router,
        "POST",
        "session/start",
        json!({"sessionId":"scope-session","context":context}),
        Some(author_token),
    )
    .await;
    let captured = ok(&router,"POST","observe",json!({"eventId":"scope-event","sessionId":"scope-session","content":"scopecheck raw capture","role":"user","source":"test","context":context}),Some(device["token"].as_str().unwrap())).await;
    assert_eq!(captured["observation"]["scope"], "project");
    let summary = ok(
        &router,
        "POST",
        "session/end",
        json!({"sessionId":"scope-session"}),
        Some(author_token),
    )
    .await;
    for token in [admin_token, author_token] {
        let rows = ok(
            &router,
            "GET",
            &format!("projects/{project_id}/memories"),
            Value::Null,
            Some(token),
        )
        .await;
        let rows = rows.as_array().unwrap();
        assert!(rows.iter().any(|m| m["memory_id"] == note["memory_id"]));
        assert!(
            rows.iter()
                .any(|m| m["kind"] == "capture" && m["summary"] == "scopecheck raw capture")
        );
        assert!(rows.iter().any(|m| m["memory_id"] == summary["memory_id"]));
        let recalled = ok(
            &router,
            "POST",
            "search",
            json!({"query":"scopecheck","context":context}),
            Some(token),
        )
        .await;
        assert!(
            recalled
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["memory_id"] == note["memory_id"])
        );
    }
    for token in [admin_token, stranger_token] {
        let rows = ok(&router, "GET", "memories", Value::Null, Some(token)).await;
        assert!(
            rows.as_array()
                .unwrap()
                .iter()
                .any(|m| m["memory_id"] == global["memory_id"])
        );
        assert!(
            !rows
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["memory_id"] == personal["memory_id"])
        );
        let (status, _) = request(
            &router,
            "GET",
            &format!("memories/{}", personal["memory_id"].as_str().unwrap()),
            Value::Null,
            Some(token),
        )
        .await;
        assert!(!(200..300).contains(&status));
    }
    let rows = ok(
        &router,
        "GET",
        "memories",
        Value::Null,
        Some(stranger_token),
    )
    .await;
    assert!(
        !rows
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["scope"] == "project")
    );
    for route in ["correct", "delete"] {
        let body = if route == "correct" {
            json!({"targetId":global["memory_id"],"content":"forbidden"})
        } else {
            json!({"id":global["memory_id"]})
        };
        let (status, _) = request(&router, "POST", route, body, Some(stranger_token)).await;
        assert!(!(200..300).contains(&status));
    }
    let authored = ok(
        &router,
        "POST",
        "correct",
        json!({"targetId":global["memory_id"],"content":"scopecheck author corrected global"}),
        Some(author_token),
    )
    .await;
    assert_eq!(authored["author"], global["author"]);
    let correction = ok(
        &router,
        "POST",
        "correct",
        json!({"targetId":authored["memory_id"],"content":"scopecheck corrected global"}),
        Some(admin_token),
    )
    .await;
    assert_eq!(correction["scope"], "global");
    ok(
        &router,
        "POST",
        "delete",
        json!({"id":correction["memory_id"]}),
        Some(admin_token),
    )
    .await;
    for method in ["GET", "POST"] {
        let (status, _) = request(&router, method, "promote", json!({}), Some(author_token)).await;
        assert!(matches!(status, 404 | 405));
        let (status, _) = request(
            &router,
            method,
            &format!("projects/{project_id}/personal-memories"),
            json!({}),
            Some(author_token),
        )
        .await;
        assert!(matches!(status, 404 | 405));
    }
}

#[tokio::test]
async fn file_http_shared_project_model() {
    let path = std::env::temp_dir().join(format!("cairn-project-http-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&path);
    scenario(app(AppState::open(&path).unwrap())).await;
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_http_shared_project_model() {
    let url = std::env::var("CAIRN_TEST_DATABASE_URL").unwrap();
    scenario(postgres_app(
        cairn_storage::PostgresStore::connect(&url).await.unwrap(),
    ))
    .await;
}
