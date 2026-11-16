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
    let owner = ok(
        &router,
        "POST",
        "auth/bootstrap",
        json!({"email":"owner@example.test","password":pw}),
        None,
    )
    .await;
    let token = owner["token"].as_str().unwrap();
    let member = ok(
        &router,
        "POST",
        "auth/register",
        json!({"email":"member@example.test","password":pw}),
        Some(token),
    )
    .await;
    let reader = ok(
        &router,
        "POST",
        "auth/register",
        json!({"email":"reader@example.test","password":pw}),
        Some(token),
    )
    .await;
    let member_token = member["token"].as_str().unwrap();
    let reader_token = reader["token"].as_str().unwrap();
    let device = ok(
        &router,
        "POST",
        "devices/register",
        json!({"name":"Member agent"}),
        Some(member_token),
    )
    .await;
    let device_token = device["token"].as_str().unwrap();
    let captured=ok(&router,"POST","observe",json!({"eventId":"no-file","sessionId":"s","content":"personal repository capture","role":"agent","source":"test","context":{"repository":"host/org/repo"}}),Some(device_token)).await;
    assert_eq!(captured["project_attachment"]["status"], "personal");
    assert_eq!(
        ok(&router, "GET", "projects", Value::Null, Some(member_token)).await,
        json!([])
    );
    let warning=ok(&router,"POST","observe",json!({"eventId":"unknown-file","sessionId":"s","content":"still personal","role":"agent","source":"test","context":{"repository":"host/org/repo","project_id":"unknown-project"}}),Some(device_token)).await;
    assert!(warning["project_attachment"]["warning"].is_string());
    assert_eq!(
        ok(&router, "GET", "projects", Value::Null, Some(member_token)).await,
        json!([])
    );
    let project = ok(
        &router,
        "POST",
        "projects/init",
        json!({"repository":"host/org/repo"}),
        Some(token),
    )
    .await;
    let id = project["project_id"].as_str().unwrap();
    assert_eq!(project["name"], "repo");
    let rename_path = format!("projects/{id}/rename");
    for who in [None, Some(member_token), Some(device_token)] {
        assert!(
            request(
                &router,
                "POST",
                &rename_path,
                json!({"name":"Refused"}),
                who
            )
            .await
            .0 >= 400
        );
    }
    let mismatch = request(
        &router,
        "POST",
        "projects/init",
        json!({"repository":"host/org/repo","project_id":id,"name":"Wrong"}),
        Some(member_token),
    )
    .await;
    assert_eq!(mismatch.0, 400);
    assert_eq!(
        ok(&router, "GET", "projects", Value::Null, Some(member_token)).await,
        json!([])
    );
    for who in [member_token, reader_token] {
        let joined = ok(
            &router,
            "POST",
            "projects/init",
            json!({"repository":"host/org/repo","project_id":id}),
            Some(who),
        )
        .await;
        assert_eq!(joined["project_id"], id);
        assert_eq!(joined["created"], false);
        assert_eq!(
            ok(&router, "GET", "projects", Value::Null, Some(who))
                .await
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    for who in [member_token, reader_token, device_token] {
        assert_eq!(
            request(
                &router,
                "POST",
                &rename_path,
                json!({"name":"Refused"}),
                Some(who)
            )
            .await
            .0,
            401
        );
    }
    let owner_device = ok(
        &router,
        "POST",
        "devices/register",
        json!({"name":"Owner agent"}),
        Some(token),
    )
    .await;
    for who in [token, owner_device["token"].as_str().unwrap()] {
        let renamed = ok(
            &router,
            "POST",
            &rename_path,
            json!({"name": "Useful project name"}),
            Some(who),
        )
        .await;
        assert_eq!(renamed["name"], "Useful project name");
        assert_eq!(renamed["project_id"], id);
    }
    assert_eq!(
        ok(&router, "GET", "projects", Value::Null, Some(member_token)).await[0]["name"],
        "Useful project name"
    );
    let audit = ok(&router, "GET", "auth/audit", Value::Null, Some(token)).await;
    assert_eq!(
        audit
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["action"] == "project_renamed")
            .count(),
        1
    );
    assert_eq!(
        request(
            &router,
            "POST",
            "projects/init",
            json!({"repository":"host/org/repo","project_id":id,"name":"repo"}),
            Some(token)
        )
        .await
        .0,
        400
    );
    let path = format!("projects/{id}");
    let content = "httpboundary complete decision payload ".repeat(40);
    let source = ok(
        &router,
        "POST",
        "remember",
        json!({"content":content,"context":{"repository":"host/org/repo","files":["src/lib.rs"]}}),
        Some(member_token),
    )
    .await;
    let source_id = source["memory_id"].as_str().unwrap();
    let preview = ok(
        &router,
        "POST",
        "promote",
        json!({"projectId":id,"memoryId":source_id,"reason":"publish","dryRun":true}),
        Some(member_token),
    )
    .await;
    assert!(preview["previewFingerprint"].is_string());
    assert_eq!(request(&router,"POST","promote",json!({"projectId":id,"memoryId":source_id,"reason":"publish","previewFingerprint":"stale-preview"}),Some(member_token)).await.0,409);
    assert_eq!(preview["content"], content);
    assert_eq!(preview["context"]["files"], json!(["src/lib.rs"]));
    assert_eq!(
        ok(
            &router,
            "GET",
            &format!("{path}/memories"),
            Value::Null,
            Some(reader_token)
        )
        .await,
        json!([])
    );
    let decision = ok(
        &router,
        "POST",
        "promote",
        json!({"projectId":id,"memoryId":source_id,"reason":"publish","previewFingerprint":preview["previewFingerprint"]}),
        Some(member_token),
    )
    .await;
    let decision_id = decision["memory_id"].as_str().unwrap();
    for who in [token, member_token, reader_token] {
        let rows = ok(
            &router,
            "GET",
            &format!("{path}/memories"),
            Value::Null,
            Some(who),
        )
        .await;
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["memory_id"], decision_id);
        assert!(rows[0]["promoted_from"].is_null());
    }
    assert_eq!(
        ok(
            &router,
            "GET",
            &format!("{path}/personal-memories"),
            Value::Null,
            Some(reader_token)
        )
        .await,
        json!([])
    );
    for (route, body) in [
        (
            "projects/verify",
            json!({"projectId":id,"memoryId":decision_id,"commit":"bad"}),
        ),
        ("correct", json!({"targetId":decision_id,"content":"bad"})),
        ("delete", json!({"id":decision_id})),
    ] {
        assert!(
            request(&router, "POST", route, body, Some(reader_token))
                .await
                .0
                >= 400
        );
    }
    let recalled = ok(
        &router,
        "POST",
        "context",
        json!({"query":"httpboundary","context":{"project_id":id,"repository":"host/org/repo"}}),
        Some(reader_token),
    )
    .await;
    assert_eq!(recalled["results"].as_array().unwrap().len(), 1);
    assert_eq!(recalled["results"][0]["source"], "project");
    assert_ne!(recalled["results"][0]["memory_id"], source_id);
    ok(
        &router,
        "POST",
        "correct",
        json!({"targetId":source_id,"content":"private source correction"}),
        Some(member_token),
    )
    .await;
    assert_eq!(
        ok(
            &router,
            "GET",
            &format!("{path}/memories"),
            Value::Null,
            Some(reader_token)
        )
        .await
        .as_array()
        .unwrap()
        .len(),
        1
    );
    let corrected = ok(
        &router,
        "POST",
        "correct",
        json!({"targetId":decision_id,"content":"httpboundary corrected publication"}),
        Some(token),
    )
    .await;
    assert_eq!(corrected["published_by"], member["account"]["person_id"]);
    assert_eq!(corrected["changed_by"], owner["account"]["person_id"]);
    let rows = ok(
        &router,
        "GET",
        &format!("{path}/memories"),
        Value::Null,
        Some(reader_token),
    )
    .await;
    assert_eq!(rows[0]["memory_id"], corrected["memory_id"]);
    let secret=ok(&router,"POST","remember",json!({"content":"secret note","context":{"repository":"host/org/repo"},"lifecycle":{"sensitivity":"secret"}}),Some(member_token)).await;
    for dry_run in [true, false] {
        assert!(request(&router,"POST","promote",json!({"projectId":id,"memoryId":secret["memory_id"],"reason":"no","dryRun":dry_run}),Some(member_token)).await.0>=400);
    }
    let ownership = format!("{path}/ownership");
    assert_eq!(
        request(
            &router,
            "POST",
            &ownership,
            json!({"personId":reader["account"]["person_id"]}),
            Some(device_token)
        )
        .await
        .0,
        403
    );
    assert!(
        request(
            &router,
            "POST",
            &ownership,
            json!({"personId":reader["account"]["person_id"]}),
            Some(member_token)
        )
        .await
        .0 >= 400
    );
    ok(
        &router,
        "POST",
        &ownership,
        json!({"personId":reader["account"]["person_id"]}),
        Some(token),
    )
    .await;
    assert_eq!(
        ok(
            &router,
            "GET",
            &format!("{path}/members"),
            Value::Null,
            Some(member_token)
        )
        .await[0]["person_id"],
        reader["account"]["person_id"]
    );
    // The former owner is still the administrator; rename has no admin bypass.
    assert_eq!(
        request(
            &router,
            "POST",
            &rename_path,
            json!({"name":"Admin override"}),
            Some(token)
        )
        .await
        .0,
        401
    );
    ok(
        &router,
        "POST",
        &rename_path,
        json!({"name":"Useful project name"}),
        Some(reader_token),
    )
    .await;
    ok(
        &router,
        "POST",
        &ownership,
        json!({"personId":owner["account"]["person_id"]}),
        Some(token),
    )
    .await;
    for route in [
        "team",
        "shares",
        "shared",
        "policy",
        "share-candidates",
        "revocations",
        "team/members",
    ] {
        for method in ["GET", "POST"] {
            assert_eq!(
                request(
                    &router,
                    method,
                    &format!("{path}/{route}"),
                    json!({}),
                    Some(token)
                )
                .await
                .0,
                404,
                "{method} {route}"
            );
        }
    }
    assert_eq!(
        request(
            &router,
            "POST",
            &format!("{path}/members"),
            json!({}),
            Some(token)
        )
        .await
        .0,
        405
    );
    let exported = ok(&router, "POST", "export", json!({}), Some(member_token)).await;
    assert!(
        exported["memories"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["published_by"].is_null())
    );
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
