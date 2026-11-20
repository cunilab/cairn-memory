//! The shared project model over HTTP: accounts, devices, membership and who sees what.

mod common;
use common::{App, PASSWORD, ids};
use serde_json::json;

#[tokio::test]
async fn shared_project_model() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let author = app.member(&admin, "author").await;
    let stranger = app.member(&admin, "stranger").await;
    let author_device = app.device(&author, "author-laptop").await;
    let stranger_device = app.device(&stranger, "stranger-laptop").await;

    // A device token cannot manage accounts or devices; a stranger bearer is a plain 401.
    let (status, body) = app
        .send("GET", "devices", json!(null), Some(&author_device))
        .await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["code"], "forbidden");
    let (status, _) = app.send("GET", "devices", json!(null), Some("nope")).await;
    assert_eq!(status, 401);
    let (status, _) = app
        .send("GET", "auth/accounts", json!(null), Some(&author))
        .await;
    assert_eq!(status, 403);

    // init creates, then joins by the committed id; a differing name is refused for a member.
    let project = app
        .post(
            "projects/init",
            json!({"repository": "github.com/acme/widgets", "name": "Widgets"}),
            &author_device,
        )
        .await;
    assert_eq!(project["created"], true);
    let id = project["project_id"].as_str().unwrap();
    let joined = app
        .post(
            "projects/init",
            json!({"project_id": id, "repository": "x", "name": "Widgets"}),
            &stranger_device,
        )
        .await;
    assert_eq!(joined["created"], false);
    let (status, _) = app
        .send(
            "POST",
            "projects/init",
            json!({"project_id": id, "repository": "x", "name": "Other"}),
            Some(&stranger_device),
        )
        .await;
    assert_eq!(status, 400);

    // Both are members; saving into the project uses the request context.
    let context =
        json!({"project_id": id, "repository": "github.com/acme/widgets", "branch": "feature-x"});
    let rule = app
        .post(
            "remember",
            json!({"kind": "rule", "title": "Use cents", "summary": "Money is integer cents.", "reason": "float drift", "context": context}),
            &author_device,
        )
        .await;
    let rule_id = rule["id"].as_str().unwrap().to_owned();
    assert_eq!(rule["deduplicated"], false);
    let personal = app
        .post(
            "remember",
            json!({"kind": "note", "title": "My scratch", "summary": "private", "scope": "personal"}),
            &author_device,
        )
        .await;
    let again = app
        .post(
            "remember",
            json!({"kind": "rule", "title": "Use cents", "summary": "Money is integer cents.", "reason": "float drift", "context": context}),
            &author_device,
        )
        .await;
    assert_eq!(
        (again["id"].as_str(), again["deduplicated"].as_bool()),
        (Some(rule_id.as_str()), Some(true))
    );

    let seen = app
        .get(&format!("projects/{id}/memories"), &stranger_device)
        .await;
    let seen_ids: Vec<&str> = seen
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(seen_ids, vec![rule_id.as_str()]);
    let (status, _) = app
        .send(
            "GET",
            &format!("memories/{}", personal["id"].as_str().unwrap()),
            json!(null),
            Some(&stranger_device),
        )
        .await;
    assert_eq!(status, 404, "another person's personal memory is not found");
    let all = app.get("memories", &stranger).await;
    assert_eq!(all.as_array().unwrap().len(), 1);

    // Branches are derived from the memories; a non-member sees no project.
    let branches = app
        .get(&format!("projects/{id}/branches"), &author_device)
        .await;
    assert_eq!(branches[0]["branch"], "feature-x");
    let outsider = app.member(&admin, "outsider").await;
    let (status, _) = app
        .send(
            "GET",
            &format!("projects/{id}/memories"),
            json!(null),
            Some(&outsider),
        )
        .await;
    assert_eq!(status, 404);
    assert_eq!(app.get("projects", &outsider).await, json!([]));

    // Conflicts come from `contradicts` links.
    let other = app
        .post("remember", json!({"kind": "rule", "title": "Use floats", "summary": "Money is a float.", "reason": "simple", "context": context, "contradicts": rule_id}), &stranger_device)
        .await;
    let conflicts = app
        .get(&format!("projects/{id}/conflicts"), &author_device)
        .await;
    assert_eq!(conflicts[0]["memory_id"], other["id"]);
    assert_eq!(conflicts[0]["contradicts_id"], rule_id);

    // Ownership: only the owner (or an admin) transfers, and only to a member.
    let members = app.get(&format!("projects/{id}/members"), &author).await;
    let stranger_person = members
        .as_array()
        .unwrap()
        .iter()
        .find(|m| !m["is_owner"].as_bool().unwrap())
        .unwrap()["person_id"]
        .clone();
    let (status, _) = app
        .send(
            "POST",
            &format!("projects/{id}/ownership"),
            json!({"personId": stranger_person}),
            Some(&stranger),
        )
        .await;
    assert_eq!(status, 403);
    app.ok(
        "POST",
        &format!("projects/{id}/ownership"),
        json!({"personId": stranger_person}),
        Some(&author),
    )
    .await;
    let listed = app.get("projects", &stranger).await;
    assert_eq!(listed[0]["role"], "owner");
    let renamed = app
        .post(
            &format!("projects/{id}/rename"),
            json!({"name": "Widgets 2"}),
            &stranger_device,
        )
        .await;
    assert_eq!(renamed["name"], "Widgets 2");
    let (status, _) = app
        .send(
            "POST",
            &format!("projects/{id}/rename"),
            json!({"name": "no"}),
            Some(&author_device),
        )
        .await;
    assert_eq!(status, 403);

    // The author can forget their own memory; it then vanishes for the member too.
    let (status, _) = app
        .send(
            "POST",
            "forget",
            json!({"id": rule_id}),
            Some(&stranger_device),
        )
        .await;
    assert_eq!(status, 404, "only the owner of a memory forgets it");
    let (status, _) = app
        .send(
            "POST",
            "delete",
            json!({"id": rule_id}),
            Some(&author_device),
        )
        .await;
    assert_eq!(status, 204);
    let seen = app
        .get(&format!("projects/{id}/memories"), &stranger_device)
        .await;
    assert!(!ids(&json!({"results": seen})).contains(&rule_id));
    let (status, body) = app
        .send("POST", "remember", json!({"kind": "rule", "title": "Use cents", "summary": "Money is integer cents.", "reason": "float drift", "context": context}), Some(&author_device))
        .await;
    assert_eq!((status, body["code"].as_str()), (409, Some("conflict")));
    let _ = PASSWORD;
    app.finish().await;
}

#[tokio::test]
async fn bad_save_input_is_a_400_with_a_message() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "d").await;
    for (body, needle) in [
        (
            json!({"kind": "rule", "title": "t", "summary": "s", "reason": "r", "colour": "red"}),
            "colour",
        ),
        (
            json!({"kind": "rule", "title": "t", "summary": "s", "reason": "r", "fields": {"symptom": "x"}}),
            "symptom",
        ),
        (
            json!({"kind": "handoff", "title": "t", "summary": "s", "reason": "r"}),
            "handoff",
        ),
        (
            json!({"kind": "rule", "title": "t", "summary": "s"}),
            "reason",
        ),
        (
            json!({"kind": "rule", "title": "t", "summary": "s", "reason": "r", "scope": "project"}),
            "project",
        ),
    ] {
        let (status, reply) = app.send("POST", "remember", body, Some(&device)).await;
        assert_eq!(status, 400, "{reply}");
        assert!(reply["error"].as_str().unwrap().contains(needle), "{reply}");
    }
    app.finish().await;
}

#[tokio::test]
async fn removed_routes_are_gone() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    for (method, path) in [
        ("GET", "/viewer"),
        ("POST", "/cairn-memory/correct"),
        ("POST", "/cairn-memory/import"),
        ("POST", "/cairn-memory/sync/pull"),
        ("GET", "/cairn-memory/similarity-reviews"),
        ("POST", "/cairn-memory/projects"),
        ("POST", "/cairn-memory/projects/verify"),
    ] {
        let response = tower::ServiceExt::oneshot(
            app.router.clone(),
            axum::http::Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {admin}"))
                .header("content-type", "application/json")
                .body(axum::body::Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
        assert!(
            matches!(response.status().as_u16(), 404 | 405),
            "{method} {path}: {}",
            response.status()
        );
    }
    app.finish().await;
}
