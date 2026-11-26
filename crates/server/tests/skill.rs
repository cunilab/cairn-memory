mod common;

use common::{App, ids};
use serde_json::{Value, json};

fn draft(name: &str, title: &str, description: &str) -> Value {
    json!({
        "name": name, "title": title, "description": description,
        "body": "# Private body marker\n\nRun the deployment. token=abc123",
        "change_note": "E2E first version"
    })
}

async fn prompt(app: &App, device: &str, session: &str, text: &str, context: Value) -> Value {
    app.post(
        "session/prompt-recall",
        json!({
            "session_id": session, "prompt": text, "context": context
        }),
        device,
    )
    .await
}

#[tokio::test]
async fn skill_archive_is_admin_only_and_preserves_versions_and_actor() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("e2e-admin").await;
    let member = app.member(&admin, "e2e-member").await;
    let device = app.device(&member, "E2E skill device").await;
    let title = "E2E rehearse Postgres promotion";
    let query = "rehearse Postgres database promotion";
    let name = "e2e-postgres-promotion";
    let draft = draft(name, title, "Check database promotion before release");
    assert_eq!(
        app.send("POST", "skills/save", draft.clone(), None).await.0,
        401
    );
    let saved = app.post("skills/save", draft.clone(), &device).await;
    let id = saved["id"].as_str().unwrap();
    let version_two = app.post("skills/save", draft, &member).await;
    assert_eq!(version_two["version"], 2);
    let get = json!({"name_or_id": name, "version": 1});
    let fetched = app.post("skills/get", get.clone(), &device).await;
    assert_eq!(fetched["version"], 1);
    assert!(!fetched["body"].as_str().unwrap().contains("abc123"));

    let path = |action: &str| format!("skills/{id}/{action}");
    for action in ["archive", "restore"] {
        assert_eq!(
            app.send("POST", &path(action), Value::Null, Some(&member))
                .await
                .0,
            403
        );
    }
    app.post(&path("archive"), Value::Null, &admin).await;
    let admin_id = app.get("auth/me", &admin).await["person_id"].clone();
    let web = app.get(&format!("skills/{id}"), &member).await;
    assert_eq!(web["archived_by"], admin_id);
    assert!(web["archived_at"].is_number());
    assert_eq!(
        ids(&app.get("skills?archived=true&offset=0", &member).await),
        [id]
    );
    assert!(
        ids(&app
            .post("skills/search", json!({"query": query}), &device)
            .await)
        .is_empty()
    );
    let (status, response) = app
        .send("POST", "skills/get", get.clone(), Some(&device))
        .await;
    assert_eq!(status, 409);
    assert_eq!(response["error"], "skill is archived");
    assert_eq!(
        prompt(&app, &device, "e2e-archived", query, json!({})).await["text"],
        ""
    );

    app.post(&path("restore"), Value::Null, &admin).await;
    assert!(
        !ids(&app
            .post("skills/search", json!({"query": query}), &device)
            .await)
        .is_empty()
    );
    assert_eq!(app.post("skills/get", get, &device).await["version"], 1);
    let rollback = app
        .post(
            &path("rollback"),
            json!({"version": 1, "change_note": "E2E rollback"}),
            &member,
        )
        .await;
    assert_eq!(rollback["version"], 3);
    assert_eq!(rollback["body"], fetched["body"]);
    assert_eq!(
        app.get(&path("history"), &member)
            .await
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert!(
        prompt(&app, &device, "e2e-archived", query, json!({})).await["text"]
            .as_str()
            .unwrap()
            .contains(title)
    );
    let actors: Vec<(String, String)> = sqlx::query_as(
        "SELECT action, person_id FROM audit_events WHERE subject_id = $1 ORDER BY action",
    )
    .bind(id)
    .fetch_all(app.db.store.pool())
    .await
    .unwrap();
    assert_eq!(
        actors,
        [
            ("skill_archived".into(), admin_id.as_str().unwrap().into()),
            ("skill_restored".into(), admin_id.as_str().unwrap().into())
        ]
    );
    app.finish().await;
}

#[tokio::test]
async fn skill_card_is_bounded_and_shares_the_session_budget_outside_chats() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("e2e-admin").await;
    let device = app.device(&admin, "E2E skill device").await;
    let account = app.get("auth/me", &admin).await;
    let person_id = account["person_id"].as_str().unwrap();
    let query = "rehearse Postgres database promotion";
    for number in 1..=2 {
        app.post(
            "skills/save",
            draft(
                &format!("e2e-promotion-{number}"),
                &format!("E2E Postgres promotion {number}"),
                "Rehearse Postgres database promotion",
            ),
            &device,
        )
        .await;
    }
    for skip in [
        "/rehearse Postgres database promotion",
        "Postgres promotion",
        "Please Postgres promotion chocolate bread dough",
        "private body marker deploy",
    ] {
        assert_eq!(
            prompt(&app, &device, "e2e-session", skip, json!({})).await["text"],
            "",
            "{skip}"
        );
    }
    let boundary = prompt(
        &app,
        &device,
        "e2e-threshold",
        "Postgres database promotion phantom bicycle",
        json!({}),
    )
    .await;
    assert!(boundary["text"].as_str().unwrap().contains("Skill:"));
    let (first, second) = tokio::join!(
        prompt(&app, &device, "e2e-session", query, json!({})),
        prompt(&app, &device, "e2e-session", query, json!({}))
    );
    let shown = format!("{first}{second}");
    assert_eq!(shown.matches("Skill:").count(), 1, "{shown}");
    assert!(shown.contains("Rehearse Postgres database promotion"));
    assert!(shown.contains("matches your prompt"));
    assert!(!shown.contains("Private body marker"));
    assert_eq!(
        prompt(&app, &device, "e2e-session", query, json!({})).await["text"],
        ""
    );
    let chat =
        json!({"chat": {"platform": "telegram", "kind": "dm", "chat_id_hash": "a".repeat(64)}});
    assert_eq!(
        prompt(
            &app,
            &device,
            "hchat_0123456789abcdef0123456789abcdef",
            query,
            chat
        )
        .await["text"],
        ""
    );

    let mut memories = Vec::new();
    for i in 0..8 {
        let id = app
            .save(
                &device,
                json!({"kind": "note", "title": format!("E2E budget row {i}"),
            "summary": "This is an unrelated listing test."}),
            )
            .await;
        memories.push(id);
    }
    for session in ["e2e-budget", "e2e-full"] {
        let ids = if session == "e2e-budget" {
            &memories[..7]
        } else {
            &memories
        };
        sqlx::query(
            "INSERT INTO session_shown (person_id, session_id, memory_id, source)
                      SELECT $1, $2, id, 'brief' FROM unnest($3::text[]) id",
        )
        .bind(person_id)
        .bind(session)
        .bind(ids)
        .execute(app.db.store.pool())
        .await
        .unwrap();
    }
    let final_card = prompt(&app, &device, "e2e-budget", query, json!({})).await;
    assert!(final_card["text"].as_str().unwrap().contains("Skill:"));
    assert_eq!(final_card["budget_spent"], true);
    assert!(
        app.db
            .store
            .push_budget_spent(person_id, "e2e-budget")
            .await
            .unwrap()
    );
    let full = prompt(&app, &device, "e2e-full", query, json!({})).await;
    assert_eq!(full["text"], "");
    assert_eq!(full["budget_spent"], true);
    app.finish().await;
}

#[tokio::test]
async fn procedure_suggestions_require_usage_and_publish_only_on_click() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("e2e-admin").await;
    let member = app.member(&admin, "e2e-member").await;
    let device = app.device(&admin, "E2E suggestion device").await;
    let mut suggestions = Vec::new();
    for (label, fetched, helpful, steps) in [
        (
            "fetched",
            5,
            0,
            json!(["Rehearse deployment", "Confirm the dashboard"]),
        ),
        ("helpful", 0, 2, json!(["Check capacity"])),
        ("low", 4, 1, json!(["Check capacity"])),
        ("empty", 9, 9, json!([])),
    ] {
        let id = app
            .save(
                &device,
                json!({
                    "kind": "procedure", "title": format!("E2E {label} procedure"),
                    "summary": "Use a test environment for these steps.",
                    "reason": "The deployment playbook is rehearsed before release.",
                    "fields": {"steps": steps}
                }),
            )
            .await;
        sqlx::query(
            "INSERT INTO usage (memory_id, fetched, helpful) VALUES ($1, $2, $3)
                      ON CONFLICT (memory_id) DO UPDATE SET fetched = $2, helpful = $3",
        )
        .bind(&id)
        .bind(fetched)
        .bind(helpful)
        .execute(app.db.store.pool())
        .await
        .unwrap();
        suggestions.push(id);
    }
    let suggestions_url = "skills/suggestions";
    let eligible = app.get(suggestions_url, &admin).await;
    assert_eq!(eligible.as_array().unwrap().len(), 2, "{eligible}");
    assert_eq!(eligible[0]["memory_id"], suggestions[1]);
    assert_eq!(eligible[1]["memory_id"], suggestions[0]);
    assert_eq!(app.get(suggestions_url, &member).await, json!([]));
    assert_eq!(app.get("skills", &admin).await["results"], json!([]));

    let path = |i: usize, action: &str| format!("{suggestions_url}/{}/{action}", suggestions[i]);
    assert_eq!(
        app.send("POST", &path(0, "create"), Value::Null, Some(&member))
            .await
            .0,
        404
    );
    assert_eq!(
        app.send("POST", &path(2, "create"), Value::Null, Some(&admin))
            .await
            .0,
        404
    );
    app.post(&path(1, "dismiss"), Value::Null, &admin).await;
    let convert = path(0, "create");
    let (created, retry) = tokio::join!(
        app.post(&convert, Value::Null, &admin),
        app.post(&convert, Value::Null, &admin)
    );
    assert_eq!(created, retry);
    assert_eq!(created["version"], 1);
    let skill = app
        .post("skills/get", json!({"name_or_id": created["id"]}), &member)
        .await;
    assert_eq!(
        skill["body"],
        "1. Rehearse deployment\n2. Confirm the dashboard"
    );
    assert_eq!(app.get(suggestions_url, &admin).await, json!([]));
    let id = created["id"].as_str().unwrap();
    app.post(&format!("skills/{id}/archive"), Value::Null, &admin)
        .await;
    assert_eq!(app.get(suggestions_url, &admin).await, json!([]));
    app.finish().await;
}
