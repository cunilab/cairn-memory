//! Capture, distillation, search and the continuity routes over HTTP, including that one
//! person's private memory never reaches another person's response.

mod common;
use common::{App, ids};
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
    // Typed lines come from the assistant, marker lines for the handoff from the person's prompts.
    let event = |id: &str, content: &str| json!({"event_id": id, "session_id": "s1", "kind": if content.starts_with("Decision") { "assistant" } else { "prompt" }, "content": content, "context": context});
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
            "observe",
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

    // The handoff is found by kind, and `memory_get` shows its next steps. Another kind finds none.
    let handoff = app
        .post(
            "search",
            json!({"query": "Handoff main", "kind": "handoff", "context": context}),
            &device,
        )
        .await;
    assert_eq!(handoff["results"].as_array().unwrap().len(), 1, "{handoff}");
    assert_eq!(handoff["results"][0]["kind"], "handoff");
    let got = app
        .post("get", json!({"ids": ids(&handoff)}), &device)
        .await;
    assert!(
        got["memories"][0]["fields"]["next_steps"][0]
            .as_str()
            .unwrap()
            .contains("TODO")
    );
    let other = app
        .post(
            "search",
            json!({"query": "Handoff main", "kind": "decision"}),
            &device,
        )
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
    let handoffs = app.get("memories?kind=handoff", &device).await;
    assert_eq!(handoffs.as_array().unwrap().len(), 1);
    assert!(
        handoffs[0]["fields"]["next_steps"]
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
        save(&app, owner, json!({"title": title, "summary": format!("{title} summary for the test"), "reason": "evidence and the test says why", "scope": scope, "kind": kind, "context": context})).await;
    }
    let (status, _) = app
        .send(
            "POST",
            "session/brief",
            json!({"session_id": "b1", "context": context}),
            None,
        )
        .await;
    assert_eq!(status, 401);
    let reply = app
        .post(
            "session/brief",
            json!({"session_id": "b1", "context": context}),
            &device,
        )
        .await;
    let text = reply["brief"].as_str().unwrap();
    assert!(text.len() <= reply["max_bytes"].as_u64().unwrap() as usize);
    for title in ["personal-visible", "project-visible"] {
        assert!(text.contains(title), "{text}");
    }
    assert!(!text.contains("private-hidden"));
    let text = app
        .post(
            "session/brief",
            json!({"session_id": "b1", "context": context}),
            &other,
        )
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
    let secret = save(&app, &admin, json!({"kind": "gotcha", "title": "Private payroll quirk", "summary": "Payroll export duplicates rows.", "reason": "seen twice", "scope": "personal", "fields": {"symptom": "duplicate rows", "fix": "Dedupe the export by id"}})).await;
    let global = save(&app, &admin, json!({"kind": "fact", "title": "Global idempotency keys", "summary": "Use idempotency keys on retries.", "reason": "payments and the test says why", "scope": "global"})).await;

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
        .post("search", json!({"query": "idempotency keys retries"}), &bob)
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
    let keys: Vec<_> = found["results"][0].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["id", "kind", "score", "summary", "title"]);
    let got = app.post("get", json!({"ids": [&queue]}), &bob_device).await;
    assert_eq!(got["memories"][0]["fields"]["alternatives"][0], "Redis");
    assert!(got["memories"][0].get("usage").is_none());
    // A link from alice's private memory to the project decision is invisible to bob: it adds
    // no hit and shows up in no `links` list.
    let linker = save(&app, &admin, json!({"kind": "gotcha", "title": "Private queue quirk", "summary": "Claims lag. is a summary of the test", "reason": "seen and the test says why", "scope": "personal", "contradicts": queue, "fields": {"symptom": "lag", "fix": "Raise the lease"}})).await;
    let linked = app
        .post(
            "search",
            json!({"query": "job queue SKIP LOCKED", "context": context}),
            &bob_device,
        )
        .await;
    assert_eq!(ids(&linked), vec![queue.clone()]);
    let theirs = app
        .post("get", json!({"ids": [&queue, &linker]}), &bob_device)
        .await;
    assert!(theirs["memories"][0].get("links").is_none(), "{theirs}");
    assert_eq!(theirs["missing"][0], linker.as_str());
    let mine = app.get(&format!("memories/{queue}"), &admin).await;
    assert_eq!(
        mine["links"][0]["relation"], "contradicts",
        "the owner of the link sees it"
    );
    let graph = app.get("graph", &bob_device).await;
    assert!(
        graph["edges"].as_array().unwrap().is_empty()
            && graph["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|n| n["memory_id"] != linker.as_str())
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
        save(&app, &admin, json!({"kind": "fact", "title": format!("Retry budget {n}"), "summary": "Retry budget is three attempts.", "reason": "ops and the test says why", "scope": "personal"})).await;
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
async fn supersede_graph_export_and_diagnose() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "d").await;
    let old = save(&app, &device, json!({"kind": "decision", "title": "Cache sessions in Redis", "summary": "Sessions live in Redis.", "reason": "speed and the test says why", "scope": "personal"})).await;
    let new = save(&app, &device, json!({"kind": "decision", "title": "Cache sessions in Postgres", "summary": "Sessions moved to Postgres.", "reason": "one datastore", "scope": "personal", "supersedes": old})).await;
    // The replaced memory leaves normal search but stays readable and listed.
    let search = app
        .post("search", json!({"query": "cache sessions"}), &device)
        .await;
    assert_eq!(ids(&search), vec![new.clone()]);
    assert!(
        search["results"][0].get("status").is_none(),
        "active cards omit status"
    );
    assert_eq!(
        app.get(&format!("memories/{old}"), &device).await["status"],
        "superseded"
    );
    let got = app.post("get", json!({"ids": [&new, &old]}), &device).await;
    assert_eq!(got["memories"][0]["links"][0]["relation"], "supersedes");
    assert_eq!(got["memories"][0]["title"], "Cache sessions in Postgres");
    assert_eq!(got["memories"][1]["status"], "superseded");

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
    assert_eq!(
        (marked["status"].as_str(), marked["id"].as_str()),
        (Some("stale"), Some(new.as_str()))
    );
    // A stale memory stays in search, and only its card carries `status`.
    let search = app
        .post("search", json!({"query": "cache sessions"}), &device)
        .await;
    assert_eq!(ids(&search), vec![new.clone()]);
    assert_eq!(search["results"][0]["status"], "stale");
    let (status, _) = app
        .send(
            "POST",
            "feedback",
            json!({"memoryId": new, "vote": "meh"}),
            Some(&device),
        )
        .await;
    assert_eq!(status, 400);
    app.finish().await;
}

#[tokio::test]
async fn get_returns_requested_records_and_names_the_rest() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let bob = app.member(&admin, "bob").await;
    let mine = save(&app, &admin, json!({"kind": "fact", "title": "Mine title for tests", "summary": "s is a summary of the test", "reason": "r and the test says why", "scope": "personal", "fields": {"source": "docs"}})).await;
    let shared = save(&app, &admin, json!({"kind": "fact", "title": "Shared title for tests", "summary": "s is a summary of the test", "reason": "r and the test says why", "scope": "global"})).await;
    let got = app
        .post("get", json!({"ids": [shared, "nope", mine]}), &admin)
        .await;
    let order: Vec<_> = got["memories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["title"].as_str().unwrap())
        .collect();
    assert_eq!(order, ["Shared title for tests", "Mine title for tests"]);
    assert_eq!(got["missing"], json!(["nope"]));
    // A repeated id is answered once, in first-seen order.
    let dup = app
        .post(
            "get",
            json!({"ids": [&shared, "nope", &shared, "nope", &mine, &mine]}),
            &admin,
        )
        .await;
    assert_eq!(dup["memories"].as_array().unwrap().len(), 2);
    assert_eq!(dup["memories"][0]["title"], "Shared title for tests");
    assert_eq!(dup["missing"], json!(["nope"]));
    let keys: Vec<_> = got["memories"][1].as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "fields",
            "id",
            "kind",
            "reason",
            "scope",
            "status",
            "summary",
            "title",
            "updated_at"
        ]
    );
    // Another person's private memory is missing for them; empty `missing` is left out.
    let theirs = app.post("get", json!({"ids": [mine]}), &bob).await;
    assert_eq!(
        (
            theirs["memories"].as_array().unwrap().len(),
            &theirs["missing"]
        ),
        (0, &json!([mine]))
    );
    assert!(got["memories"][0].get("fields").is_none());
    for ids in [json!([]), json!(vec!["a"; 11]), json!(["x".repeat(129)])] {
        let (status, _) = app
            .send("POST", "get", json!({"ids": ids}), Some(&admin))
            .await;
        assert_eq!(status, 400);
    }
    app.finish().await;
}

#[tokio::test]
async fn removed_routes_and_unknown_search_keys_are_refused() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    for path in [
        "smart-search",
        "recall",
        "why-changed",
        "resume",
        "unresolved",
        "session/start",
        "supersede",
        "contradict",
        "links",
        "links/remove",
    ] {
        let (status, _) = app.send("POST", path, json!({}), Some(&admin)).await;
        assert_eq!(status, 404, "{path}");
    }
    let (status, _) = app
        .send(
            "POST",
            "search",
            json!({"query": "x", "sessionId": "s1"}),
            Some(&admin),
        )
        .await;
    assert_eq!(status, 422, "unknown search keys are refused");
    app.finish().await;
}

#[tokio::test]
async fn job_queue_is_admin_only_and_idle_worker_is_a_no_op() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let user = app.member(&admin, "user").await;
    let device = app.device(&admin, "d").await;
    save(&app, &device, json!({"kind": "fact", "title": "A fact title for tests", "summary": "s is a summary of the test", "reason": "r and the test says why", "scope": "personal"})).await;
    let queue = app.get("admin/jobs", &admin).await;
    assert_eq!(
        (queue["pending"].as_i64(), queue["quarantined"].as_i64()),
        (Some(2), Some(0))
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

#[tokio::test]
async fn observe_is_typed_and_deduped() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "laptop").await;
    for bad in [
        json!({"event_id": "x", "session_id": "s", "kind": "prompt", "content": "c", "role": "user"}),
        json!({"event_id": "x", "session_id": "s", "kind": "tool", "content": "c"}),
        json!({"event_id": "x", "session_id": "s", "content": "c"}),
    ] {
        let (status, body) = app.send("POST", "observe", bad, Some(&device)).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (400, Some("validation")),
            "{body}"
        );
    }
    let command = json!({"event_id": "c1", "session_id": "s", "kind": "command", "ok": false, "content": "cargo test\nexit 101: boom"});
    let first = app.post("observe", command.clone(), &device).await;
    assert_eq!(first["duplicate"], false);
    let mut again = command;
    again["event_id"] = "c2".into();
    let again = app.post("observe", again, &device).await;
    assert_eq!(
        (again["accepted"].as_bool(), again["duplicate"].as_bool()),
        (Some(true), Some(true))
    );
    assert_eq!(again["event_id"], "c2");

    let ok: Option<bool> = sqlx::query_scalar("SELECT ok FROM observations WHERE event_id = 'c1'")
        .fetch_one(app.db.store.pool())
        .await
        .unwrap();
    assert_eq!(ok, Some(false));
    app.finish().await;
}

#[tokio::test]
async fn session_topic_returns_file_notes_and_brief_records_recent_files() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "laptop").await;
    let id = save(
        &app,
        &device,
        json!({"kind": "gotcha", "scope": "personal", "title": "Queue lock", "summary": "Hold the lock first.", "reason": "r and the test says why",
               "fields": {"symptom": "hang", "cause": "order", "fix": "lock first"},
               "context": {"files": ["src/queue.rs"]}}),
    )
    .await;
    let reply = app
        .post(
            "session/topic",
            json!({"path": "src/queue.rs", "context": {}}),
            &device,
        )
        .await;
    assert_eq!(reply["notes"].as_array().unwrap().len(), 1, "{reply}");
    assert_eq!(
        (
            reply["notes"][0]["id"].as_str(),
            reply["notes"][0]["title"].as_str(),
            reply["notes"][0]["summary"].as_str()
        ),
        (
            Some(id.as_str()),
            Some("Queue lock"),
            Some("Hold the lock first.")
        )
    );
    let none = app
        .post(
            "session/topic",
            json!({"path": "src/queue", "context": {}}),
            &device,
        )
        .await;
    assert_eq!(none["notes"], json!([]));
    let (status, _) = app
        .send(
            "POST",
            "session/topic",
            json!({"path": " ", "context": {}}),
            Some(&device),
        )
        .await;
    assert_eq!(status, 400);
    let (status, _) = app
        .send(
            "POST",
            "session/topic",
            json!({"path": "src/queue.rs"}),
            None,
        )
        .await;
    assert_eq!(status, 401);
    let used: i64 = sqlx::query_scalar("SELECT coalesce(sum(fetched), 0)::bigint FROM usage")
        .fetch_one(app.db.store.pool())
        .await
        .unwrap();
    assert_eq!(used, 0, "a topic note is not a fetch");

    let brief = app
        .post(
            "session/brief",
            json!({"session_id": "s9", "context": {"branch": "main"}, "recent_files": ["src/queue.rs", "token=abc123"]}),
            &device,
        )
        .await;
    assert!(brief["brief"].as_str().unwrap().len() <= 1200);
    assert_eq!(brief["max_bytes"], 1200);
    let files: serde_json::Value =
        sqlx::query_scalar("SELECT context->'recent_files' FROM sessions WHERE session_id = 's9'")
            .fetch_one(app.db.store.pool())
            .await
            .unwrap();
    assert_eq!(files, json!(["src/queue.rs", "token=[REDACTED]"]));
    let (status, _) = app
        .send(
            "POST",
            "session/brief",
            json!({"context": {}}),
            Some(&device),
        )
        .await;
    assert!((400..500).contains(&status), "a brief needs a session id");
    app.finish().await;
}

#[tokio::test]
async fn search_budget_caps_bytes_and_kind_filters() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    for n in 0..10 {
        save(&app, &admin, json!({"kind": "fact", "title": format!("Ledger audit trail {n}"), "summary": "Ledger audit trail entries ".repeat(8).trim(), "reason": "ops and the test says why", "scope": "personal"})).await;
    }
    save(&app, &admin, json!({"kind": "rule", "title": "Ledger audit rule", "summary": "Audit the ledger weekly.", "reason": "ops and the test says why", "scope": "personal"})).await;
    let all = app
        .post(
            "search",
            json!({"query": "ledger audit trail", "limit": 20}),
            &admin,
        )
        .await;
    assert_eq!(all["results"].as_array().unwrap().len(), 11, "{all}");
    let small = app
        .post(
            "search",
            json!({"query": "ledger audit trail", "limit": 20, "budget": 200}),
            &admin,
        )
        .await;
    assert!(
        small.to_string().len() <= 800 + 64,
        "{}",
        small.to_string().len()
    );
    let cards = small["results"].as_array().unwrap().len();
    assert!((1..11).contains(&cards), "{small}");
    assert_eq!(small["more"].as_u64(), Some(11 - cards as u64));
    let rules = app
        .post(
            "search",
            json!({"query": "ledger audit trail", "kind": "rule"}),
            &admin,
        )
        .await;
    assert_eq!(rules["results"].as_array().unwrap().len(), 1, "{rules}");
    assert_eq!(rules["results"][0]["kind"], "rule");
    app.finish().await;
}

#[tokio::test]
async fn only_search_and_get_count_usage() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "laptop").await;
    let id = save(
        &app,
        &device,
        json!({"kind": "gotcha", "scope": "personal", "title": "Usage probe", "summary": "Ledger audit trail. is a summary of the test", "reason": "r and the test says why",
               "fields": {"symptom": "s", "cause": "c", "fix": "f"}, "context": {"files": ["src/usage.rs"]}}),
    )
    .await;
    let fetched = || async {
        sqlx::query_scalar::<_, i64>("SELECT coalesce(sum(fetched), 0)::bigint FROM usage")
            .fetch_one(app.db.store.pool())
            .await
            .unwrap()
    };
    app.post(
        "session/topic",
        json!({"path": "src/usage.rs", "context": {}}),
        &device,
    )
    .await;
    app.post(
        "session/brief",
        json!({"session_id": "s1", "context": {}, "recent_files": ["src/usage.rs"]}),
        &device,
    )
    .await;
    app.get("memories", &admin).await;
    app.get("graph", &admin).await;
    assert_eq!(fetched().await, 0, "topic, brief and web reads never count");
    app.post("search", json!({"query": "ledger audit"}), &device)
        .await;
    assert_eq!(fetched().await, 1);
    app.post("get", json!({"ids": [&id]}), &device).await;
    assert_eq!(fetched().await, 2);
    app.finish().await;
}

#[tokio::test]
async fn brief_uses_recent_files() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "laptop").await;
    let project = app
        .post(
            "projects/init",
            json!({"repository": "r", "name": "R"}),
            &device,
        )
        .await;
    let pid = project["project_id"].as_str().unwrap();
    let context = |file: &str| json!({"project_id": pid, "repository": "r", "files": [file]});
    for (title, file) in [
        ("Queue lock order", "src/jobs/queue.rs"),
        ("Docs build flag", "docs/build.md"),
    ] {
        save(&app, &device, json!({"kind": "gotcha", "scope": "project", "title": title, "summary": "Mind it. is a summary of the test", "reason": "r and the test says why",
            "fields": {"symptom": "s", "cause": "c", "fix": "f"}, "context": context(file)})).await;
    }
    let brief = |recent: serde_json::Value| {
        let (app, device) = (&app, &device);
        async move {
            let reply = app
                .post(
                    "session/brief",
                    json!({"session_id": "s1", "context": {"project_id": pid}, "recent_files": recent}),
                    device,
                )
                .await;
            reply["brief"].as_str().unwrap().to_owned()
        }
    };
    let position = |text: &str, title: &str| {
        text.find(title)
            .unwrap_or_else(|| panic!("{title} in {text}"))
    };
    let touching_queue = brief(json!(["src/jobs/worker.rs"])).await;
    assert!(
        position(&touching_queue, "Queue lock order")
            < position(&touching_queue, "Docs build flag")
    );
    let touching_docs = brief(json!(["docs/build.md"])).await;
    assert!(
        position(&touching_docs, "Docs build flag") < position(&touching_docs, "Queue lock order")
    );
    app.finish().await;
}

#[tokio::test]
async fn brief_offers_a_check_and_repo_state_is_device_only_and_default_branch_only() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "laptop").await;
    let project = app
        .post(
            "projects/init",
            json!({"repository": "github.com/acme/widgets", "name": "Widgets"}),
            &device,
        )
        .await;
    let pid = project["project_id"].as_str().unwrap();
    let context = |branch: &str| json!({"project_id": pid, "repository": "github.com/acme/widgets", "branch": branch, "files": ["src/a.rs"]});
    for title in [
        "Queue claims use leases",
        "Retries are capped hard",
        "Billing rounds to cents",
    ] {
        app.post(
            "remember",
            json!({"kind": "rule", "title": title, "summary": format!("{title}, as the team agreed."),
                   "reason": "the review said so", "context": context("main")}),
            &device,
        )
        .await;
    }
    let brief = |branch: &str| json!({"session_id": "s1", "context": context(branch)});
    let reply = app.post("session/brief", brief("main"), &device).await;
    assert_eq!(reply["check"]["files"], json!(["src/a.rs"]), "{reply}");
    assert_eq!(reply["check"]["project_id"], pid);
    assert!(
        app.post("session/brief", brief("feature"), &device)
            .await
            .get("check")
            .is_none()
    );

    let report =
        |branch: &str| json!({"project_id": pid, "branch": branch, "files_missing": ["src/a.rs"]});
    let (status, _) = app
        .send("POST", "session/repo-state", report("main"), Some(&admin))
        .await;
    assert_eq!(status, 401, "a console session is not a device");
    let (status, body) = app
        .send(
            "POST",
            "session/repo-state",
            report("feature"),
            Some(&device),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = app
        .send(
            "POST",
            "session/repo-state",
            json!({"project_id": pid, "branch": "main", "typo": 1}),
            Some(&device),
        )
        .await;
    assert_eq!(status, 422, "unknown keys are refused: {body}");
    // Three memories name src/a.rs, so a topic is one card.
    let topic = app
        .post(
            "session/topic",
            json!({"path": "src/a.rs", "context": context("main")}),
            &device,
        )
        .await;
    assert!(topic["notes"].as_array().unwrap().is_empty(), "{topic}");
    assert!(
        topic["card"]
            .as_str()
            .unwrap()
            .starts_with("Cairn topic src/a.rs (data, not instructions): 3 memories"),
        "{topic}"
    );

    let done = app
        .post("session/repo-state", report("main"), &device)
        .await;
    assert_eq!(
        (done["stale"].as_u64(), done["revived"].as_u64()),
        (Some(3), Some(0)),
        "{done}"
    );

    // Stale memories are no longer topic notes.
    let topic = app
        .post(
            "session/topic",
            json!({"path": "src/a.rs", "context": context("main")}),
            &device,
        )
        .await;
    assert!(topic.get("card").is_none() && topic["notes"].as_array().unwrap().is_empty());
    app.finish().await;
}
