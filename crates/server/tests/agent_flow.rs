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
        "evidence": ["apps/docs/content/users/index.mdx"],
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

/// Title and summary sizes past the suggestion save as sent with a size hint and no trust change; past the
/// hard cap they are refused. Cards cut to the suggestion; memory_get returns the full text.
#[tokio::test]
async fn long_titles_and_summaries_save_with_a_size_hint_up_to_the_cap() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("sizes").await;
    let device = app.device(&admin, "sizes-laptop").await;
    let summary = format!("Long summaries are kept as sent. {}", "x".repeat(231));
    assert_eq!(summary.chars().count(), 264);
    let saved = app
        .post(
            "remember",
            note("fact", "Long summaries are kept with a size hint", &summary),
            &device,
        )
        .await;
    assert_eq!(saved["status"], "active", "{saved}");
    assert_eq!(
        saved["hints"],
        json!(["summary is 264 characters; 240 suggested"])
    );
    assert!(saved.get("improve").is_none(), "{saved}");
    let id = saved["id"].as_str().unwrap();
    let rank: String = sqlx::query_scalar("SELECT rank FROM memories WHERE memory_id = $1")
        .bind(id)
        .fetch_one(app.db.store.pool())
        .await
        .unwrap();
    assert_eq!(rank, "normal", "a size hint alone changes no trust");
    let got = app.post("get", json!({"ids": [id]}), &device).await;
    assert_eq!(got["memories"][0]["summary"], summary);
    let found = app
        .post(
            "search",
            json!({"query": "long summaries kept size hint"}),
            &device,
        )
        .await;
    let card = found["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|card| card["id"] == id)
        .unwrap_or_else(|| panic!("{found}"));
    assert_eq!(
        card["summary"],
        format!("{}\u{2026}", summary.chars().take(239).collect::<String>())
    );

    let (status, body) = app
        .send(
            "POST",
            "remember",
            note("fact", "A summary over the hard cap", &"y".repeat(1001)),
            Some(&device),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body.to_string()
            .contains("summary is 1001 characters, limit 1000: cut 1"),
        "{body}"
    );
    app.finish().await;
}

#[tokio::test]
async fn coaches_each_save_and_clean_supersede_replaces_candidate() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("quality").await;
    let device = app.device(&admin, "quality-laptop").await;
    let project = app
        .post(
            "projects/init",
            json!({"repository": "github.com/acme/save-coaching", "name": "E2E Batch coaching"}),
            &device,
        )
        .await;
    let context = json!({"project_id": project["project_id"]});
    let weak = note("fact", "Fix", "A short summary");
    let mut thin = note(
        "decision",
        "Keep the import review window",
        "Reviewers need time to check imported notes.",
    );
    thin["reason"] = json!("Verified.");
    let batch = app
        .post(
            "remember",
            json!({
                "context": context,
                "items": [weak, thin, compact_note()]
            }),
            &device,
        )
        .await;
    assert_eq!(statuses(&batch), ["saved", "saved", "saved"]);
    let [first, second, third] = batch["results"].as_array().unwrap().as_slice() else {
        panic!("{batch}");
    };
    assert_eq!(
        first["hints"],
        json!([
            "Use at least three specific words in the title to name the lesson.",
            "Write a summary of at least five words that adds to the title."
        ])
    );
    assert_eq!(
        second["hints"],
        json!([
            "Reason should explain why this is true or was chosen in at least 20 characters.",
            "Add decision context or list the alternatives you considered."
        ])
    );
    assert!(
        third.get("hints").is_none() && third.get("improve").is_none(),
        "{third}"
    );
    for item in [first, second] {
        let id = item["id"].as_str().unwrap();
        assert_eq!(
            item["improve"],
            format!("Improve it: save again with supersedes={id}.")
        );
        let rank: String = sqlx::query_scalar("SELECT rank FROM memories WHERE memory_id = $1")
            .bind(id)
            .fetch_one(app.db.store.pool())
            .await
            .unwrap();
        assert_eq!(rank, "candidate");
    }

    let mut correction = note(
        "fact",
        "Retain receipts for reconciliation",
        "Reconciliation reads the receipts retained by the import.",
    );
    correction["context"] = context;
    correction["supersedes"] = first["id"].clone();
    let saved = app.post("remember", correction, &device).await;
    assert_eq!(
        saved,
        json!({"id": saved["id"], "status": "active", "deduplicated": false})
    );
    assert_ne!(saved["id"], first["id"]);
    let got = app
        .post("get", json!({"ids": [first["id"], saved["id"]]}), &device)
        .await;
    assert_eq!(got["memories"][0]["status"], "superseded");
    assert_eq!(got["memories"][1]["trust"], "normal");
    app.finish().await;
}

#[tokio::test]
async fn duplicate_hints_only_consider_active_memories_in_the_same_owner_and_scope() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("duplicate-owner").await;
    let owner = app.device(&admin, "owner-laptop").await;
    let member = app.member(&admin, "duplicate-member").await;
    let teammate = app.device(&member, "member-laptop").await;
    let project = app.post("projects/init", json!({
        "repository": "github.com/acme/coaching-duplicates", "name": "E2E Duplicate coaching"
    }), &owner).await;
    let context = json!({"project_id": project["project_id"]});
    let mut first = note(
        "fact",
        "Webhook handlers suppress repeat deliveries",
        "An event ID table prevents double billing.",
    );
    first["context"] = context.clone();
    let original = app.post("remember", first.clone(), &owner).await;
    assert!(original.get("hints").is_none(), "{original}");

    // These notes have identical lexeme sets and different identities.
    let mut different_owner = first.clone();
    different_owner["summary"] = json!("The event ID table prevents double billing.");
    let saved = app.post("remember", different_owner, &teammate).await;
    assert!(saved.get("hints").is_none(), "{saved}");
    let mut different_scope = first.clone();
    different_scope["scope"] = json!("personal");
    different_scope["summary"] = json!("An event ID table prevents double billing!");
    let saved = app.post("remember", different_scope, &owner).await;
    assert!(saved.get("hints").is_none(), "{saved}");
    app.post(
        "feedback",
        json!({"id": original["id"], "vote": "wrong"}),
        &owner,
    )
    .await;
    first["summary"] = json!("The event ID table prevents double billing!");
    let saved = app.post("remember", first, &owner).await;
    assert!(saved.get("hints").is_none(), "{saved}");
    app.finish().await;
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
                note("fact", "Too long summary", &"x".repeat(1001)),
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
    assert_eq!(
        error["error"],
        "items[1]: validation error: summary is 1001 characters, limit 1000: cut 1, or move extra context into reason or detail"
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

/// A single save has no schema `required` array. `SaveInput` still rejects a
/// missing title: `kind`, `title` and `summary` have no serde default.
#[tokio::test]
async fn single_save_missing_title_is_rejected() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let member = app.member(&admin, "member").await;
    let device = app.device(&member, "laptop").await;
    let (status, body) = app
        .send(
            "POST",
            "remember",
            json!({
                "kind": "fact",
                "summary": "A single save with no title must be rejected.",
                "reason": "The server still requires title when items is absent."
            }),
            Some(&device),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["code"], "validation");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|text| text.contains("title")),
        "{body}"
    );
    assert_eq!(
        app.get("memories", &device).await.as_array().unwrap().len(),
        0
    );
    app.finish().await;
}
