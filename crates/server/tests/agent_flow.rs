//! `memory_save` with `items`: up to three notes, partial success, batch rejected otherwise.

mod common;
use common::App;
use serde_json::{Value, json};

fn note(kind: &str, title: &str, summary: &str) -> Value {
    json!({
        "kind": kind,
        "title": title,
        "summary": summary,
        "reason": "The test keeps this note so the batch can be checked."
    })
}

fn full_note() -> Value {
    json!({
        "kind": "fact",
        "title": "Full save input note",
        "summary": "A full item may set fields, evidence and scope.",
        "reason": "The server validates each item as a SaveInput.",
        "fields": {"source": "the release notes"},
        "evidence": ["docs/plan/0.1.9/03-save-ask.md"],
        "scope": "personal"
    })
}

fn compact_note() -> Value {
    json!({
        "kind": "fact",
        "title": "Compact schema note",
        "summary": "A compact item carries only the typed note fields.",
        "reason": "The published schema repeats names and types."
    })
}

fn statuses(body: &Value) -> Vec<&str> {
    body["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["status"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn save_items_partial_success() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let member = app.member(&admin, "member").await;
    let device = app.device(&member, "laptop").await;
    let rows = |token: &str| {
        let app = &app;
        let token = token.to_owned();
        async move { app.get("memories", &token).await.as_array().unwrap().len() }
    };

    let three = app
        .post(
            "remember",
            json!({"items": [
                compact_note(),
                full_note(),
                note("rule", "One save call only", "The agent saves at most three notes in one call.")
            ]}),
            &device,
        )
        .await;
    assert_eq!(statuses(&three), ["saved", "saved", "saved"]);
    assert!(
        three["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| { item["id"].as_str().is_some_and(|id| !id.is_empty()) })
    );
    assert_eq!(rows(&device).await, 3);

    let mixed = app
        .post(
            "remember",
            json!({"items": [
                note("fact", "Second compact note", "Another compact note is stored on its own."),
                {"kind": "fact", "title": "x", "summary": "y", "reason": "z"},
                {
                    "kind": "rule",
                    "title": "Second full save note",
                    "summary": "Another full note includes evidence and a scope.",
                    "reason": "An invalid sibling must not roll it back.",
                    "fields": {"exceptions": "none"},
                    "evidence": ["crates/server/src/routes/memory.rs"],
                    "scope": "personal"
                }
            ]}),
            &device,
        )
        .await;
    assert_eq!(statuses(&mixed), ["saved", "error", "saved"]);
    let error = &mixed["results"][1];
    assert!(error.get("id").is_none() || error["id"].is_null());
    assert!(
        error["error"].as_str().is_some_and(|text| !text.is_empty()),
        "{error}"
    );
    assert_eq!(rows(&device).await, 5);

    let four = json!({"items": [
        note("fact", "Fourth a", "This fourth note must not be stored at all."),
        note("fact", "Fourth b", "This fourth note must not be stored at all."),
        note("fact", "Fourth c", "This fourth note must not be stored at all."),
        note("fact", "Fourth d", "This fourth note must not be stored at all.")
    ]});
    let (status, body) = app.send("POST", "remember", four, Some(&device)).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["code"], "validation");
    assert_eq!(rows(&device).await, 5);

    let both = json!({
        "title": "Top level title",
        "items": [note("fact", "Combined note", "A top-level title must reject the whole call.")]
    });
    let (status, body) = app.send("POST", "remember", both, Some(&device)).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["code"], "validation");
    assert!(body["error"].as_str().unwrap().contains("title"), "{body}");
    assert_eq!(rows(&device).await, 5);
    app.finish().await;
}
