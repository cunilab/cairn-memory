mod common;

use cairn_core::memory::validate::DETAIL_MAX;
use common::App;
use serde_json::json;

#[tokio::test]
async fn detail_is_saved_and_searchable_but_requires_an_explicit_single_get() {
    let Some(app) = App::new().await else { return };
    let owner = app.admin("detail-owner").await;
    let device = app.device(&owner, "detail-agent").await;
    let mut body = json!({
        "kind": "preference",
        "title": "Prefer a clear catalog",
        "summary": "Use a compact catalog to keep session outputs readable.",
        "reason": "A short catalog is easier to scan.",
        "scope": "personal",
        "context": {"files": ["src/catalog.rs"]}
    });
    let id = app.save(&device, body.clone()).await;
    let unique = "astronomical chromolithography";
    let detail = format!("{unique} {}", "📌".repeat(DETAIL_MAX - unique.len() - 1));
    body["detail"] = json!(format!("{detail}📌"));
    let (status, invalid) = app
        .send("POST", "remember", body.clone(), Some(&device))
        .await;
    assert_eq!(status, 400);
    assert!(
        invalid["error"]
            .as_str()
            .unwrap()
            .contains("detail must be at most 8000")
    );
    body["detail"] = json!(detail);
    let dedup = app.post("remember", body.clone(), &device).await;
    assert_eq!(dedup["id"], id);
    assert_eq!(dedup["deduplicated"], true);

    for request in [
        json!({"ids": [&id]}),
        json!({"ids": [&id], "detail": false}),
    ] {
        let response = app.post("get", request, &device).await;
        let record = &response["memories"][0];
        assert_eq!(record["detail_chars"], DETAIL_MAX);
        assert!(record.get("detail").is_none());
        assert!(!response.to_string().contains(unique));
    }
    let request = json!({"ids": [&id], "detail": true});
    let full = app.post("get", request.clone(), &device).await;
    assert!(full["memories"][0]["detail"].as_str() == Some(detail.as_str()));
    let (status, error) = app
        .send(
            "POST",
            "get",
            json!({"ids": [&id, &id], "detail": true}),
            Some(&device),
        )
        .await;
    assert_eq!(status, 400);
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("one memory at a time")
    );

    let search = app.post("search", json!({"query": unique}), &device).await;
    assert_eq!(search["results"][0]["id"], id, "{search}");
    assert!(!search.to_string().contains(unique));
    let brief = app
        .post(
            "session/brief",
            json!({"session_id": "s_detail", "context": {}}),
            &device,
        )
        .await;
    assert!(
        brief["brief"]
            .as_str()
            .unwrap()
            .contains("Prefer a clear catalog")
    );
    assert!(!brief.to_string().contains(unique));
    let topic = app
        .post("session/topic", json!({"path": "src/catalog.rs"}), &device)
        .await;
    assert_eq!(topic["notes"][0]["id"], id);
    assert!(!topic.to_string().contains(unique));

    body["detail"] = json!("Ordinary duplicate attempts leave prior detail in place.");
    app.post("remember", body.clone(), &device).await;
    let after_duplicate = app.post("get", request.clone(), &device).await;
    assert!(after_duplicate["memories"][0]["detail"].as_str() == Some(detail.as_str()));
    body["supersedes"] = json!(id);
    body["detail"] = json!("Corrected detail with password=fake_secret_value");
    let correction = app.post("remember", body, &device).await;
    assert_eq!(correction["id"], id);
    let corrected = app.post("get", request, &device).await;
    assert_eq!(
        corrected["memories"][0]["detail"],
        "Corrected detail with password=[REDACTED]"
    );
    app.finish().await;
}
