//! Capture, distillation, search and the continuity routes over HTTP, including that one
//! person's private memory never reaches another person's response.

mod common;
use common::{App, ids, titles};
use serde_json::json;

async fn save(app: &App, token: &str, body: serde_json::Value) -> String {
    app.post("remember", body, token).await["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn session_distills_into_notes_and_a_handoff() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "laptop").await;
    let context = json!({"repository": "r", "branch": "main", "files": ["src/queue.rs"]});
    app.post(
        "session/start",
        json!({"sessionId": "s1", "context": context}),
        &device,
    )
    .await;
    let event = |id: &str, content: &str| json!({"eventId": id, "sessionId": "s1", "role": "assistant", "source": "hook", "content": content, "context": context, "importance": "high"});
    let first = app
        .post(
            "observe",
            event(
                "e1",
                "Decision: use SKIP LOCKED for the queue\nReason: one datastore",
            ),
            &device,
        )
        .await;
    assert_eq!(
        (first["accepted"].as_bool(), first["duplicate"].as_bool()),
        (Some(true), Some(false))
    );
    let replay = app
        .post(
            "sync/push",
            event(
                "e1",
                "Decision: use SKIP LOCKED for the queue\nReason: one datastore",
            ),
            &device,
        )
        .await;
    assert_eq!(replay["duplicate"], true);
    let (status, body) = app
        .send("POST", "observe", event("e1", "different"), Some(&device))
        .await;
    assert_eq!(status, 409, "{body}");
    app.post(
        "observe",
        event("e2", "TODO: the retry path is untested"),
        &device,
    )
    .await;

    // A console session is not an agent: captures take a device token only.
    let (status, _) = app
        .send(
            "POST",
            "observe",
            event("e9", "from the console"),
            Some(&admin),
        )
        .await;
    assert_eq!(status, 401);
    // Raw captures are never recalled.
    let nothing = app
        .post("search", json!({"query": "retry path untested"}), &device)
        .await;
    assert!(ids(&nothing).is_empty(), "{nothing}");

    assert_eq!(
        app.post("session/checkpoint", json!({"sessionId": "s1"}), &device)
            .await["status"],
        "stored"
    );
    app.settle().await;
    let ended = app
        .post("session/end", json!({"sessionId": "s1"}), &device)
        .await;
    assert_eq!(
        ended["status"], "empty",
        "the checkpoint already distilled everything"
    );

    let resume = app
        .post("resume", json!({"limit": 5, "context": context}), &device)
        .await;
    assert_eq!(resume["results"].as_array().unwrap().len(), 1, "{resume}");
    assert_eq!(resume["results"][0]["kind"], "handoff");
    assert!(
        resume["results"][0]["fields"]["next_steps"][0]
            .as_str()
            .unwrap()
            .contains("TODO")
    );
    let unresolved = app.post("unresolved", json!({"limit": 5}), &device).await;
    assert_eq!(ids(&unresolved), ids(&resume));
    // Another session or branch does not see the handoff when the filter names it.
    let other = app
        .post("resume", json!({"sessionId": "s2"}), &device)
        .await;
    assert!(ids(&other).is_empty());

    let notes = app.get("memories?kind=decision", &device).await;
    assert_eq!(notes.as_array().unwrap().len(), 1);
    assert_eq!(notes[0]["scope"], "personal");
    let found = app
        .post("search", json!({"query": "SKIP LOCKED queue"}), &device)
        .await;
    assert_eq!(
        ids(&found),
        vec![notes[0]["id"].as_str().unwrap().to_owned()]
    );

    // A second distill replaces the handoff in place instead of adding one.
    app.post(
        "observe",
        event("e3", "Next step: wire the retry metric"),
        &device,
    )
    .await;
    app.post("session/end", json!({"sessionId": "s1"}), &device)
        .await;
    app.settle().await;
    let resume = app
        .post("resume", json!({"limit": 5, "context": context}), &device)
        .await;
    assert_eq!(resume["results"].as_array().unwrap().len(), 1);
    assert!(
        resume["results"][0]["fields"]["next_steps"]
            .to_string()
            .contains("retry metric")
    );
    app.finish().await;
}

#[tokio::test]
async fn brief_shows_the_caller_their_own_rules_only() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let other = app.member(&admin, "other").await;
    let project = app
        .post(
            "projects/init",
            json!({"repository": "brief-repo", "name": "Brief"}),
            &admin,
        )
        .await;
    let id = project["project_id"].as_str().unwrap();
    let context = json!({"project_id": id, "repository": "brief-repo", "branch": "feature"});
    let device = app.device(&admin, "brief").await;
    for (title, scope, kind, owner) in [
        ("personal-visible", "personal", "preference", &admin),
        ("private-hidden", "personal", "preference", &other),
        ("project-visible", "project", "rule", &admin),
    ] {
        // Saving with a project context joins the project (as `init` would), so the other
        // person saves without one and stays a non-member.
        let context = if owner == &other {
            json!({})
        } else {
            context.clone()
        };
        save(&app, owner, json!({"title": title, "summary": title, "reason": "evidence", "scope": scope, "kind": kind, "context": context})).await;
    }
    let (status, _) = app
        .send("POST", "session/brief", json!({"context": context}), None)
        .await;
    assert_eq!(status, 401);
    let reply = app
        .post("session/brief", json!({"context": context}), &device)
        .await;
    let text = reply["brief"].as_str().unwrap();
    assert!(text.len() <= reply["max_bytes"].as_u64().unwrap() as usize);
    for title in ["personal-visible", "project-visible"] {
        assert!(text.contains(title), "{text}");
    }
    assert!(!text.contains("private-hidden"));
    let text = app
        .post("session/brief", json!({"context": context}), &other)
        .await["brief"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!text.contains("project-visible") && !text.contains("personal-visible"));
    app.finish().await;
}

#[tokio::test]
async fn search_ranks_filters_and_never_leaks() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let bob = app.member(&admin, "bob").await;
    let project = app
        .post(
            "projects/init",
            json!({"repository": "r", "name": "R"}),
            &admin,
        )
        .await;
    let pid = project["project_id"].as_str().unwrap();
    let context = json!({"project_id": pid, "repository": "r"});

    let queue = save(&app, &admin, json!({"kind": "decision", "title": "Use Postgres SKIP LOCKED for the job queue", "summary": "Jobs are claimed from a table.", "reason": "one datastore", "scope": "project", "context": context, "fields": {"alternatives": ["Redis"]}})).await;
    let secret = save(&app, &admin, json!({"kind": "gotcha", "title": "Private payroll quirk", "summary": "Payroll export duplicates rows.", "reason": "seen twice", "scope": "personal", "fields": {"symptom": "duplicate rows"}})).await;
    let global = save(&app, &admin, json!({"kind": "fact", "title": "Global idempotency keys", "summary": "Use idempotency keys on retries.", "reason": "payments", "scope": "global"})).await;

    // The owner finds all three; bob finds the project memory (as a member), not the personal one.
    let mine = app
        .post("search", json!({"query": "payroll export rows"}), &admin)
        .await;
    assert_eq!(ids(&mine), vec![secret.clone()]);
    let bobs = app
        .post("search", json!({"query": "payroll export rows"}), &bob)
        .await;
    assert!(ids(&bobs).is_empty(), "{bobs}");
    assert!(
        app.send(
            "GET",
            &format!("memories/{secret}"),
            json!(null),
            Some(&bob)
        )
        .await
        .0 == 404
    );
    let global_hit = app
        .post(
            "context",
            json!({"query": "idempotency keys retries"}),
            &bob,
        )
        .await;
    assert_eq!(ids(&global_hit), vec![global.clone()]);
    // Bob joins the project; now he finds the decision, with its reason and fields.
    let bob_device = app.device(&bob, "bob").await;
    app.post(
        "projects/init",
        json!({"project_id": pid, "repository": "r"}),
        &bob_device,
    )
    .await;
    let found = app
        .post(
            "search",
            json!({"query": "job queue SKIP LOCKED", "context": context}),
            &bob_device,
        )
        .await;
    assert_eq!(ids(&found), vec![queue.clone()]);
    assert_eq!(found["results"][0]["fields"]["alternatives"][0], "Redis");
    assert!(found["results"][0].get("usage").is_none());
    // A link from alice's private memory to the project decision is invisible to bob: it adds
    // no hit and shows up in no `links` list.
    app.post(
        "links",
        json!({"sourceId": secret, "relation": "relates", "targetId": queue}),
        &admin,
    )
    .await;
    let linked = app
        .post(
            "search",
            json!({"query": "job queue SKIP LOCKED", "context": context}),
            &bob_device,
        )
        .await;
    assert_eq!(ids(&linked), vec![queue.clone()]);
    assert!(linked["results"][0].get("links").is_none(), "{linked}");
    let mine = app.get(&format!("memories/{queue}"), &admin).await;
    assert_eq!(
        mine["links"][0]["relation"], "relates",
        "the owner of the link sees it"
    );
    let graph = app.get("graph", &bob_device).await;
    assert!(
        graph["edges"].as_array().unwrap().is_empty()
            && graph["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|n| n["memory_id"] != secret.as_str())
    );
    // A project filter for another project hides it.
    let elsewhere = app
        .post(
            "search",
            json!({"query": "job queue SKIP LOCKED", "context": {"project_id": "other"}}),
            &bob_device,
        )
        .await;
    assert!(ids(&elsewhere).is_empty());
    // Weak or unrelated queries return nothing rather than the nearest rows.
    for query in ["kubernetes ingress controller", "the and of"] {
        assert!(
            ids(&app.post("search", json!({"query": query}), &admin).await).is_empty(),
            "{query}"
        );
    }
    // limit trims and `more` counts what was cut.
    for n in 0..3 {
        save(&app, &admin, json!({"kind": "fact", "title": format!("Retry budget {n}"), "summary": "Retry budget is three attempts.", "reason": "ops", "scope": "personal"})).await;
    }
    let cut = app
        .post(
            "search",
            json!({"query": "retry budget attempts", "limit": 2}),
            &admin,
        )
        .await;
    assert_eq!(
        (
            cut["results"].as_array().unwrap().len(),
            cut["more"].as_u64()
        ),
        (2, Some(1))
    );
    assert_eq!(
        app.send("POST", "search", json!({"query": " "}), Some(&admin))
            .await
            .0,
        400
    );
    app.finish().await;
}

#[tokio::test]
async fn why_changed_graph_export_and_diagnose() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "d").await;
    let old = save(&app, &device, json!({"kind": "decision", "title": "Cache sessions in Redis", "summary": "Sessions live in Redis.", "reason": "speed", "scope": "personal"})).await;
    let new = save(&app, &device, json!({"kind": "decision", "title": "Cache sessions in Postgres", "summary": "Sessions moved to Postgres.", "reason": "one datastore", "scope": "personal", "supersedes": old})).await;
    // The replaced memory leaves normal search but stays readable and listed.
    let search = app
        .post("search", json!({"query": "cache sessions"}), &device)
        .await;
    assert_eq!(ids(&search), vec![new.clone()]);
    assert_eq!(
        app.get(&format!("memories/{old}"), &device).await["status"],
        "superseded"
    );
    let why = app
        .post("why-changed", json!({"query": "sessions cache"}), &device)
        .await;
    assert_eq!(ids(&why), vec![new.clone()]);
    assert_eq!(why["results"][0]["links"][0]["relation"], "supersedes");
    assert_eq!(titles(&why), vec!["Cache sessions in Postgres"]);

    let graph = app.get("graph", &device).await;
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(graph["edges"][0]["relation"], "supersedes");
    assert_eq!(
        (
            graph["edges"][0]["source_id"].as_str(),
            graph["edges"][0]["target_id"].as_str()
        ),
        (Some(new.as_str()), Some(old.as_str()))
    );
    let (status, _) = app
        .send("GET", "graph?cap=5", json!(null), Some(&device))
        .await;
    assert_eq!(status, 400);

    let export = app.post("export", json!({}), &device).await;
    assert_eq!(
        (
            export["memories"].as_array().unwrap().len(),
            export["links"].as_array().unwrap().len()
        ),
        (2, 1)
    );
    let diagnose = app.get("diagnose", &device).await;
    assert_eq!(diagnose["memories"], 2);
    assert_eq!(diagnose["by_status"]["superseded"], 1);

    // Feedback: a wrong vote from the owner makes a memory stale and ranks it lower.
    let marked = app
        .post(
            "feedback",
            json!({"memoryId": new, "vote": "wrong"}),
            &device,
        )
        .await;
    assert_eq!(marked["status"], "stale");
    let (status, _) = app
        .send(
            "POST",
            "feedback",
            json!({"memoryId": new, "vote": "meh"}),
            Some(&device),
        )
        .await;
    assert_eq!(status, 400);
    // Unlinking the supersession restores the old memory.
    app.post(
        "links/remove",
        json!({"sourceId": new, "relation": "supersedes", "targetId": old}),
        &device,
    )
    .await;
    assert_eq!(
        app.get(&format!("memories/{old}"), &device).await["status"],
        "active"
    );
    app.finish().await;
}

#[tokio::test]
async fn job_queue_is_admin_only_and_idle_worker_is_a_no_op() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let user = app.member(&admin, "user").await;
    let device = app.device(&admin, "d").await;
    save(&app, &device, json!({"kind": "fact", "title": "A fact", "summary": "s", "reason": "r", "scope": "personal"})).await;
    let queue = app.get("admin/jobs", &admin).await;
    assert_eq!(
        (queue["pending"].as_i64(), queue["quarantined"].as_i64()),
        (Some(1), Some(0))
    );
    assert_eq!(
        app.send("GET", "admin/jobs", json!(null), Some(&user))
            .await
            .0,
        403
    );
    assert_eq!(
        app.send("GET", "admin/jobs", json!(null), Some(&device))
            .await
            .0,
        403
    );
    // No model: the worker settles the embed job without embedding anything.
    app.settle().await;
    let queue = app.get("admin/jobs", &admin).await;
    assert_eq!(queue["pending"], 0);
    assert_eq!(
        app.send(
            "POST",
            "admin/jobs/missing/requeue",
            json!(null),
            Some(&admin)
        )
        .await
        .0,
        404
    );
    app.finish().await;
}
