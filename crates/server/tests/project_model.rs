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

async fn distillation_scenario(router: Router) {
    let account = ok(
        &router,
        "POST",
        "auth/bootstrap",
        json!({"email":"distill@example.test","password":"correct horse battery staple"}),
        None,
    )
    .await;
    let token = account["token"].as_str().unwrap();
    let device = ok(&router, "POST", "devices/register", json!({"deviceId":"distill-device","personId":account["account"]["person_id"],"name":"Distillation tests"}), Some(token)).await;
    let device_token = device["token"].as_str().unwrap();
    let other = ok(
        &router,
        "POST",
        "auth/register",
        json!({"email":"other@example.test","password":"correct horse battery staple"}),
        Some(token),
    )
    .await;
    let other_token = other["token"].as_str().unwrap();
    let project = ok(
        &router,
        "POST",
        "projects/init",
        json!({"repository":"example.test/distill","name":"Distill"}),
        Some(token),
    )
    .await;
    let project_id = project["project_id"].as_str().unwrap();
    for (session, context, scope) in [
        (
            "project-work",
            json!({"project_id":project_id,"repository":"example.test/distill","files":["src/cache.rs"],"branch":"main"}),
            "project",
        ),
        (
            "private-work",
            json!({"repository":"example.test/uninitialized","files":["src/cache.rs"],"branch":"main"}),
            "personal",
        ),
        (
            "project-work-handoff",
            json!({"repository":"example.test/uninitialized","files":["src/cache.rs"],"branch":"main"}),
            "personal",
        ),
    ] {
        ok(
            &router,
            "POST",
            "session/start",
            json!({"sessionId":session,"context":context}),
            Some(token),
        )
        .await;
        let event = format!("{session}-event");
        ok(&router, "POST", "sync/push", json!({"sessionId":session,"eventId":event,"role":"user","source":"test","context":context,"content":"Decision: Keep bounded writes\nReason: Reduce contention\nAlternatives: unbounded writes\nGotcha: Stale cache\nCause: missing invalidation\nFix: clear cache\nProcedure: Verify writes\nStep: cargo test cache\nRule: Scrub secrets\nExceptions: none\nFact: Cache has one writer\nStatement: one writer\nSource: src/cache.rs\nNext: verify restart\nUnresolved: restart untested\nAn ordinary fact is not a typed note."}), Some(device_token)).await;
        let summary = ok(
            &router,
            "POST",
            "session/end",
            json!({"sessionId":session}),
            Some(token),
        )
        .await;
        assert_eq!(summary["kind"], "session_summary");
        assert_eq!(summary["scope"], scope);
        assert_eq!(summary["evidence"], json!([event]));
        let rows = ok(&router, "GET", "memories", Value::Null, Some(token)).await;
        let notes: Vec<_> = rows
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["session_id"] == session && m["kind"] != "capture")
            .cloned()
            .collect();
        assert_eq!(
            notes.len(),
            7,
            "one session note, one handoff and five typed notes: {notes:?}"
        );
        for kind in [
            "session_summary",
            "handoff",
            "decision",
            "gotcha",
            "procedure",
            "rule",
            "fact",
        ] {
            let note = notes.iter().find(|m| m["kind"] == kind).unwrap();
            assert_eq!(note["scope"], scope);
            assert_eq!(note["source_event_ids"], json!([event]));
            assert_eq!(note["evidence"], json!([event]));
            assert_eq!(note["applies_to"], json!(["src/cache.rs", "main"]));
            assert!(
                note["tags"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("work-note"))
            );
            assert!(!note["title"].as_str().unwrap().is_empty());
        }
        let decision = notes.iter().find(|m| m["kind"] == "decision").unwrap();
        assert_eq!(decision["reason"], "Reduce contention");
        assert_eq!(decision["alternatives"], json!(["unbounded writes"]));
        assert!(decision["consequences"].is_null());
        let gotcha = notes.iter().find(|m| m["kind"] == "gotcha").unwrap();
        assert_eq!(gotcha["reason"], "");
        assert_eq!(gotcha["cause"], "missing invalidation");
        assert_eq!(gotcha["fix"], "clear cache");
        assert!(gotcha["symptom"].is_null());
        let procedure = notes.iter().find(|m| m["kind"] == "procedure").unwrap();
        assert_eq!(procedure["steps"], json!(["cargo test cache"]));
        let rule = notes.iter().find(|m| m["kind"] == "rule").unwrap();
        assert!(rule["rule"].is_null());
        assert_eq!(rule["exceptions"], "none");
        let fact = notes.iter().find(|m| m["kind"] == "fact").unwrap();
        assert_eq!(fact["statement"], "one writer");
        assert_eq!(fact["source"], "src/cache.rs");
        let handoff = notes.iter().find(|m| m["kind"] == "handoff").unwrap();
        assert!(!handoff["next_steps"].as_array().unwrap().is_empty());
        assert!(!handoff["unresolved_work"].as_array().unwrap().is_empty());
        assert!(
            !handoff["summary"]
                .as_str()
                .unwrap()
                .contains("ordinary fact")
        );
        ok(
            &router,
            "POST",
            "session/end",
            json!({"sessionId":session}),
            Some(token),
        )
        .await;
        let again = ok(&router, "GET", "memories", Value::Null, Some(token)).await;
        let repeated: Vec<_> = again
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["session_id"] == session && m["kind"] != "capture")
            .cloned()
            .collect();
        assert_eq!(
            notes, repeated,
            "repeat end must preserve IDs, records and timestamps"
        );
        for note in &notes {
            let (status, _) = request(
                &router,
                "GET",
                &format!("memories/{}", note["memory_id"].as_str().unwrap()),
                Value::Null,
                Some(other_token),
            )
            .await;
            assert_eq!(
                status, 400,
                "unrelated person cannot read automatic work notes"
            );
        }
        if scope == "project" {
            ok(
                &router,
                "POST",
                "projects/init",
                json!({"project_id":project_id,"repository":"example.test/distill"}),
                Some(other_token),
            )
            .await;
            let shared = ok(
                &router,
                "GET",
                &format!("projects/{project_id}/memories"),
                Value::Null,
                Some(other_token),
            )
            .await;
            for note in &notes {
                assert!(
                    shared
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|m| m["memory_id"] == note["memory_id"])
                );
            }
        }
        // A correction hides the derived note without changing its source captures.
        let corrected = ok(
            &router,
            "POST",
            "correct",
            json!({"targetId":decision["memory_id"],"content":"Use bounded transactions instead"}),
            Some(token),
        )
        .await;
        ok(&router, "POST", "sync/push", json!({"sessionId":session,"eventId":format!("{session}-late"),"role":"tool","source":"test","content":"Fact: restart verified","context":context}), Some(device_token)).await;
        ok(
            &router,
            "POST",
            "session/end",
            json!({"sessionId":session}),
            Some(token),
        )
        .await;
        let refreshed = ok(&router, "GET", "memories", Value::Null, Some(token)).await;
        assert!(
            !refreshed
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["memory_id"] == decision["memory_id"])
        );
        assert!(
            refreshed
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["memory_id"] == corrected["memory_id"])
        );
        assert_eq!(
            refreshed
                .as_array()
                .unwrap()
                .iter()
                .filter(|m| m["session_id"] == session && m["kind"] == "handoff")
                .count(),
            1
        );
        // Project-note forgetting preserves shared captures and sibling knowledge.
        ok(
            &router,
            "POST",
            "forget",
            json!({"id":gotcha["memory_id"]}),
            Some(token),
        )
        .await;
        ok(
            &router,
            "POST",
            "session/end",
            json!({"sessionId":session}),
            Some(token),
        )
        .await;
        let retained = ok(&router, "GET", "memories", Value::Null, Some(token)).await;
        assert!(
            !retained
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["memory_id"] == gotcha["memory_id"])
        );
        if scope == "project" {
            assert!(
                retained
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["memory_id"] == rule["memory_id"])
            );
            assert!(
                retained
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["kind"] == "capture" && m["source_event_ids"] == json!([event]))
            );
        }
        // Explicit source forgetting suppresses every derivation of that capture.
        ok(&router, "POST", "forget", json!({"id":event}), Some(token)).await;
        ok(
            &router,
            "POST",
            "session/end",
            json!({"sessionId":session}),
            Some(token),
        )
        .await;
        let retained = ok(&router, "GET", "memories", Value::Null, Some(token)).await;
        assert!(!retained.as_array().unwrap().iter().any(|m| {
            m["source_event_ids"]
                .as_array()
                .unwrap()
                .contains(&json!(event))
        }));
        if scope == "project" {
            let current_summary = retained
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["memory_id"] == summary["memory_id"])
                .unwrap();
            let current_handoff = retained
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["session_id"] == session && m["kind"] == "handoff")
                .unwrap();
            let late_id = format!("{session}-late");
            let current_fact = retained
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["kind"] == "fact" && m["source_event_ids"] == json!([late_id]))
                .unwrap();
            for hidden in [current_handoff, current_summary] {
                ok(
                    &router,
                    "POST",
                    "forget",
                    json!({"id":hidden["memory_id"]}),
                    Some(token),
                )
                .await;
                let rows = ok(&router, "GET", "memories", Value::Null, Some(token)).await;
                assert!(
                    !rows
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|m| m["memory_id"] == hidden["memory_id"])
                );
                assert!(
                    rows.as_array()
                        .unwrap()
                        .iter()
                        .any(|m| m["memory_id"] == current_fact["memory_id"])
                );
                assert!(
                    rows.as_array().unwrap().iter().any(
                        |m| m["kind"] == "capture" && m["source_event_ids"] == json!([late_id])
                    )
                );
            }
        }
    }
}

#[tokio::test]
async fn file_http_session_distillation() {
    let path = std::env::temp_dir().join(format!("cairn-distill-http-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&path);
    distillation_scenario(app(AppState::open(&path).unwrap())).await;
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
#[ignore = "requires a fresh PostgreSQL database in CAIRN_TEST_DATABASE_URL"]
async fn postgres_http_session_distillation() {
    let url = std::env::var("CAIRN_TEST_DATABASE_URL").unwrap();
    distillation_scenario(postgres_app(
        cairn_storage::PostgresStore::connect(&url).await.unwrap(),
    ))
    .await;
}
