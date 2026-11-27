//! HTTP checks for session provenance and pin reorder conflicts.

mod common;
use common::App;
use serde_json::json;

#[tokio::test]
async fn remember_keeps_a_session_link_and_a_stale_pin_order_conflicts() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("extra-admin").await;
    let device = app.device(&admin, "laptop").await;
    app.post(
        "observe",
        json!({
            "event_id": "e1",
            "session_id": "s1",
            "kind": "prompt",
            "content": "note",
            "context": {"branch": "main"}
        }),
        &device,
    )
    .await;
    let saved = app
        .post(
            "remember",
            json!({
                "kind": "rule",
                "title": "Linked session rule",
                "summary": "The save names the session that created it.",
                "reason": "the test needs a reason that is long enough",
                "session_id": "s1"
            }),
            &device,
        )
        .await;
    let id = saved["id"].as_str().unwrap();
    let view = app.get(&format!("memories/{id}"), &admin).await;
    assert_eq!(view["provenance"]["session_id"], "s1");
    assert_eq!(view["provenance"]["role"], "saved");
    assert_eq!(view["can_pin"], true);

    let member = app.member(&admin, "extra-member").await;
    let (status, _) = app
        .send("GET", &format!("memories/{id}"), json!(null), Some(&member))
        .await;
    assert_eq!(status, 404);

    app.post(
        &format!("memories/{id}/pin"),
        json!({"pinned": true}),
        &admin,
    )
    .await;
    let page = app
        .get("console/memories?pinned=true&pageSize=25", &admin)
        .await;
    let position = page["items"][0]["pin_position"].as_i64().unwrap();
    let (status, body) = app
        .send(
            "POST",
            "memories/pins/order",
            json!({"scope": "personal", "order": [{"id": id, "position": position + 1}]}),
            Some(&admin),
        )
        .await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["code"], "conflict");
    assert_eq!(body["order"][0]["position"], position);
    app.finish().await;
}
