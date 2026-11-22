//! Agent instructions over HTTP: the global texts, a project's override, who may change them, the
//! 600-byte cap, the audit trail, and the texts that ride with the session brief.

mod common;
use common::App;
use serde_json::{Value, json};

fn text(reply: &Value, field: &str) -> (String, String) {
    (
        reply[field]["text"].as_str().unwrap().to_owned(),
        reply[field]["source"].as_str().unwrap().to_owned(),
    )
}

#[tokio::test]
async fn global_texts_default_set_cap_clear_and_audit() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let member = app.member(&admin, "member").await;
    let device = app.device(&member, "laptop").await;

    // Anyone signed in reads them; nothing is set, so the built-in text applies.
    for token in [&member, &device] {
        let start = app.get("agent-instructions", token).await;
        assert_eq!(text(&start, "session_instruction").1, "default");
        assert!(
            text(&start, "session_instruction")
                .0
                .starts_with("Cairn is active")
        );
        assert_eq!(start["session_instruction"]["own"], Value::Null);
        assert_eq!(start["max_bytes"], 600);
        assert!(text(&start, "stop_nudge").0.contains("{files}"));
    }

    // Only an admin web session writes: a member gets 403, a device token the sign-in code.
    let change = json!({"session_instruction": "Use Cairn first."});
    let (status, _) = app
        .send("POST", "agent-instructions", change.clone(), Some(&member))
        .await;
    assert_eq!(status, 403);
    let (status, body) = app
        .send("POST", "agent-instructions", change.clone(), Some(&device))
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (401, Some("session_required"))
    );

    // 600 bytes pass, 601 are refused with a clear message and nothing is stored.
    let (status, body) = app
        .send(
            "POST",
            "agent-instructions",
            json!({"stop_nudge": "x".repeat(601)}),
            Some(&admin),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("stop_nudge is 601 bytes; the limit is 600"),
        "{body}"
    );
    let now = app
        .post(
            "agent-instructions",
            json!({"stop_nudge": "y".repeat(600)}),
            &admin,
        )
        .await;
    assert_eq!(text(&now, "stop_nudge").1, "global");

    // Set, read by a member, change again, then clear: back to the built-in text.
    let set = app.post("agent-instructions", change, &admin).await;
    assert_eq!(
        text(&set, "session_instruction"),
        ("Use Cairn first.".into(), "global".into())
    );
    let seen = app.get("agent-instructions", &member).await;
    assert_eq!(text(&seen, "session_instruction").0, "Use Cairn first.");
    app.post(
        "agent-instructions",
        json!({"session_instruction": "Search before you edit."}),
        &admin,
    )
    .await;
    let cleared = app
        .post(
            "agent-instructions",
            json!({"session_instruction": "  ", "stop_nudge": ""}),
            &admin,
        )
        .await;
    assert_eq!(text(&cleared, "session_instruction").1, "default");
    assert_eq!(text(&cleared, "stop_nudge").1, "default");
    assert_eq!(cleared["session_instruction"]["own"], Value::Null);

    // Every change left an audit event with the old and the new text; an unchanged save left none.
    app.post(
        "agent-instructions",
        json!({"session_instruction": ""}),
        &admin,
    )
    .await;
    let audit = app.get("auth/audit?limit=50", &admin).await;
    let changes: Vec<&Value> = audit
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["action"] == "agent_instructions_changed"
                && e["metadata"]["field"] == "session_instruction"
        })
        .collect();
    assert_eq!(changes.len(), 3, "{audit}");
    // Events of one second have no fixed order, so compare as a set.
    let mut pairs: Vec<(String, String)> = changes
        .iter()
        .map(|e| {
            assert_eq!(e["metadata"]["scope"], "global");
            (
                e["metadata"]["old"].as_str().unwrap().to_owned(),
                e["metadata"]["new"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    pairs.sort();
    let expected = [
        ("", "Use Cairn first."),
        ("Search before you edit.", ""),
        ("Use Cairn first.", "Search before you edit."),
    ];
    assert_eq!(pairs, expected.map(|(a, b)| (a.to_owned(), b.to_owned())));
    app.finish().await;
}

#[tokio::test]
async fn a_project_override_wins_and_clearing_falls_back_to_the_global_text() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let owner = app.member(&admin, "owner").await;
    let other = app.member(&admin, "other").await;
    let stranger = app.member(&admin, "stranger").await;
    let owner_device = app.device(&owner, "owner-laptop").await;
    let other_device = app.device(&other, "other-laptop").await;
    let stranger_device = app.device(&stranger, "stranger-laptop").await;
    let project = app
        .post(
            "projects/init",
            json!({"repository": "github.com/acme/widgets", "name": "Widgets"}),
            &owner_device,
        )
        .await;
    let id = project["project_id"].as_str().unwrap().to_owned();
    app.post(
        "projects/init",
        json!({"project_id": id, "repository": "x", "name": "Widgets"}),
        &other_device,
    )
    .await;
    let path = format!("projects/{id}/agent-instructions");
    let brief = |token: String| {
        let (app, id) = (&app, id.clone());
        async move {
            app.post(
                "session/brief",
                json!({"session_id": "s1", "context": {"project_id": id, "repository": "x"}}),
                &token,
            )
            .await
        }
    };

    app.post(
        "agent-instructions",
        json!({"session_instruction": "Global line."}),
        &admin,
    )
    .await;
    // No override yet: the project shows the global text, with the built-in one for the other field.
    let start = app.get(&path, &owner).await;
    assert_eq!(
        text(&start, "session_instruction"),
        ("Global line.".into(), "global".into())
    );
    assert_eq!(text(&start, "stop_nudge").1, "default");
    assert_eq!(start["session_instruction"]["own"], Value::Null);

    // Members read, only the owner (or an admin) writes; a stranger does not even see the project.
    assert_eq!(
        app.send("GET", &path, json!(null), Some(&other)).await.0,
        200
    );
    let (status, body) = app
        .send(
            "POST",
            &path,
            json!({"session_instruction": "Hijack."}),
            Some(&other),
        )
        .await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(
        app.send("GET", &path, json!(null), Some(&stranger)).await.0,
        404
    );
    assert_eq!(
        app.send("POST", &path, json!({"stop_nudge": "x"}), Some(&stranger))
            .await
            .0,
        403,
        "a stranger is refused like any non-owner"
    );
    assert_eq!(
        app.send(
            "POST",
            &path,
            json!({"stop_nudge": "x"}),
            Some(&owner_device)
        )
        .await
        .0,
        401
    );
    let (status, body) = app
        .send(
            "POST",
            &path,
            json!({"stop_nudge": "z".repeat(601)}),
            Some(&owner),
        )
        .await;
    assert_eq!(status, 400, "{body}");

    // The override wins, field by field, and rides with the brief for a member only.
    let set = app
        .post(
            &path,
            json!({"session_instruction": "Widgets: search Cairn."}),
            &owner,
        )
        .await;
    assert_eq!(
        text(&set, "session_instruction"),
        ("Widgets: search Cairn.".into(), "project".into())
    );
    assert_eq!(set["session_instruction"]["inherited"], "Global line.");
    assert_eq!(text(&set, "stop_nudge").1, "default");
    let member_brief = brief(other_device.clone()).await;
    assert_eq!(
        member_brief["instructions"]["session_instruction"],
        "Widgets: search Cairn."
    );
    assert!(
        member_brief["instructions"]["stop_nudge"]
            .as_str()
            .unwrap()
            .contains("{files}")
    );
    let outsider = brief(stranger_device).await;
    assert_eq!(
        outsider["instructions"]["session_instruction"], "Global line.",
        "an override is for members"
    );

    // Clearing the override falls back to the global text, then to the default.
    let cleared = app
        .post(&path, json!({"session_instruction": ""}), &owner)
        .await;
    assert_eq!(
        text(&cleared, "session_instruction"),
        ("Global line.".into(), "global".into())
    );
    assert_eq!(
        brief(owner_device.clone()).await["instructions"]["session_instruction"],
        "Global line."
    );
    app.post(
        "agent-instructions",
        json!({"session_instruction": ""}),
        &admin,
    )
    .await;
    let again = app.get(&path, &owner).await;
    assert_eq!(text(&again, "session_instruction").1, "default");

    // An admin who is not a member can change it; the change is in the audit of whoever made it.
    let by_admin = app
        .post(
            &path,
            json!({"stop_nudge": "Admin says: save. {files}"}),
            &admin,
        )
        .await;
    assert_eq!(
        text(&by_admin, "stop_nudge"),
        ("Admin says: save. {files}".into(), "project".into())
    );
    let audit = app.get("auth/audit?limit=50", &owner).await;
    let mine: Vec<&Value> = audit
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["action"] == "agent_instructions_changed")
        .collect();
    assert_eq!(mine.len(), 2, "set and clear by the owner: {audit}");
    for event in &mine {
        assert_eq!(event["subject_id"], id.as_str());
        assert_eq!(event["metadata"]["scope"], "project");
        assert_eq!(event["metadata"]["field"], "session_instruction");
    }
    let mut pairs: Vec<(Value, Value)> = mine
        .iter()
        .map(|e| (e["metadata"]["old"].clone(), e["metadata"]["new"].clone()))
        .collect();
    pairs.sort_by_key(|(old, _)| old.as_str().unwrap().len());
    assert_eq!(
        pairs,
        [
            (json!(""), json!("Widgets: search Cairn.")),
            (json!("Widgets: search Cairn."), json!(""))
        ]
    );
    app.finish().await;
}
