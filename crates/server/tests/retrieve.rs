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
        ended["status"], "queued",
        "the first end queues final distill for handoff and digest"
    );
    app.settle().await;

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

    // A second distill adds the next handoff in the chain. An edit makes it at least as rich.
    app.post(
        "observe",
        event("e3", "next: wire the retry metric"),
        &device,
    )
    .await;
    app.post(
        "observe",
        json!({
            "event_id": "e4",
            "session_id": "s1",
            "kind": "edit",
            "content": "Edit src/queue.rs\nx",
            "context": context
        }),
        &device,
    )
    .await;
    app.post("session/end", json!({"sessionId": "s1"}), &device)
        .await;
    app.settle().await;
    let handoffs = app.get("memories?kind=handoff", &device).await;
    let handoffs = handoffs.as_array().unwrap();
    assert_eq!(handoffs.len(), 2);
    let active = handoffs
        .iter()
        .find(|memory| memory["status"] == "active")
        .unwrap();
    assert!(
        active["fields"]["next_steps"]
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
        (
            "Keep personal-visible preference",
            "personal",
            "preference",
            &admin,
        ),
        (
            "Keep private-hidden preference",
            "personal",
            "preference",
            &other,
        ),
        ("Keep project-visible rule", "project", "rule", &admin),
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
    assert_eq!(
        keys,
        [
            "id", "kind", "project", "score", "summary", "title", "trust", "why"
        ]
    );
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
async fn export_diagnose_delete_are_gone() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "d").await;
    for (method, path) in [("POST", "export"), ("GET", "diagnose"), ("POST", "delete")] {
        let (status, _) = app.send(method, path, json!({}), Some(&device)).await;
        assert_eq!(status, 404, "{method} /{path} is gone");
    }
    app.finish().await;
}

#[tokio::test]
async fn supersede_and_graph() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "d").await;
    let old = save(&app, &device, json!({"kind": "decision", "title": "Cache sessions in Redis", "summary": "Sessions live in Redis.", "reason": "speed and the test says why", "scope": "personal"})).await;
    let new = save(&app, &device, json!({"kind": "decision", "title": "Cache sessions in Postgres", "summary": "Sessions moved to Postgres.", "reason": "one datastore", "scope": "personal", "supersedes": old})).await;
    // Both stay in search. The replacement ranks above the superseded card.
    let search = app
        .post("search", json!({"query": "cache sessions"}), &device)
        .await;
    assert_eq!(ids(&search)[0], new);
    assert!(ids(&search).contains(&old));
    let old_card = search["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|card| card["id"] == old)
        .unwrap();
    assert_eq!(old_card["label"], format!("replaced by {new}"));
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
    assert_eq!(ids(&search)[0], new);
    assert!(ids(&search).contains(&old));
    assert_eq!(search["results"][0]["status"], "stale");
    // Stale is never unexplained: reply, card and record all name the cause.
    assert_eq!(marked["stale_cause"], "feedback");
    assert_eq!(search["results"][0]["stale_cause"], "feedback");
    let got = app.post("get", json!({"ids": [&new]}), &device).await;
    assert_eq!(got["memories"][0]["stale_cause"], "feedback");
    // A helpful vote cannot lift a stale from the repo check, and the reply says why.
    sqlx::query("UPDATE memories SET stale_cause = 'missing' WHERE memory_id = $1")
        .bind(&new)
        .execute(app.db.store.pool())
        .await
        .unwrap();
    let helped = app
        .post(
            "feedback",
            json!({"memoryId": new, "vote": "helpful"}),
            &device,
        )
        .await;
    assert_eq!(
        (helped["status"].as_str(), helped["stale_cause"].as_str()),
        (Some("stale"), Some("missing"))
    );
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
            "trust",
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
        401,
        "a device token is not a console session"
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
    let mut again = command.clone();
    again["event_id"] = "c2".into();
    let again = app.post("observe", again, &device).await;
    // A later execution of the same command is kept, so the digest can report it.
    assert_eq!(
        (again["accepted"].as_bool(), again["duplicate"].as_bool()),
        (Some(true), Some(false))
    );
    assert_eq!(again["event_id"], "c2");
    let replay = app.post("observe", command, &device).await;
    assert_eq!(replay["duplicate"], true);

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
        json!({"kind": "gotcha", "scope": "personal", "title": "Queue lock order", "summary": "Hold the queue lock before claiming jobs.", "reason": "r and the test says why",
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
            Some("Queue lock order"),
            Some("Hold the queue lock before claiming jobs.")
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
    assert!(
        brief["brief"].as_str().unwrap().len()
            <= cairn_core::retrieve::brief::LEGACY_SESSION_BRIEF_MAX_BYTES
    );
    assert_eq!(
        brief["max_bytes"],
        cairn_core::retrieve::brief::LEGACY_SESSION_BRIEF_MAX_BYTES
    );
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
            json!({"session_id": " ", "context": {}}),
            Some(&device),
        )
        .await;
    assert_eq!(status, 400, "an explicitly empty session ID is invalid");
    let untracked = app
        .post("session/brief", json!({"context": {}}), &device)
        .await;
    assert_eq!(
        untracked["max_bytes"],
        cairn_core::retrieve::brief::LEGACY_SESSION_BRIEF_MAX_BYTES
    );
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
async fn hook_replies_count_shown_and_agent_search_and_get_count_used() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "laptop").await;
    let id = save(
        &app,
        &device,
        json!({"kind": "preference", "scope": "personal", "title": "Track usage probe", "summary": "Ledger audit trail. is a summary of the test", "reason": "r and the test says why",
               "detail": "Ledger audit history.", "context": {"files": ["src/usage.rs"]}}),
    )
    .await;
    let usage = || async { app.get(&format!("memories/{id}"), &admin).await["usage"].clone() };
    let brief = app
        .post(
            "session/brief",
            json!({"session_id": "s1", "context": {}, "recent_files": ["src/usage.rs"]}),
            &device,
        )
        .await;
    assert!(brief["brief"].as_str().unwrap().contains(&id));
    let after_brief = usage().await;
    assert_eq!(after_brief["shown"], 1);
    assert_eq!(after_brief["fetched"], 0);
    assert!(after_brief["last_shown_at"].is_number());
    assert!(after_brief["last_used_at"].is_null());
    let topic = app
        .post(
            "session/topic",
            json!({"path": "src/usage.rs", "context": {}}),
            &device,
        )
        .await;
    assert_eq!(topic["notes"][0]["id"], id);
    app.get("memories", &admin).await;
    app.get("graph", &admin).await;
    let after_topic = usage().await;
    assert_eq!(after_topic["shown"], 2);
    assert_eq!(after_topic["fetched"], 0);
    let search = app
        .post("search", json!({"query": "ledger audit"}), &device)
        .await;
    assert_eq!(search["results"][0]["trust"], "normal");
    let after_search = usage().await;
    assert_eq!(after_search["shown"], 2);
    assert_eq!(after_search["fetched"], 1);
    let get = app.post("get", json!({"ids": [&id]}), &device).await;
    assert_eq!(get["memories"][0]["trust"], "normal · used 1×");
    let after_get = usage().await;
    assert_eq!(after_get["fetched"], 2);
    assert_eq!(after_get["detail_opened"], 0);
    app.post("get", json!({"ids": [&id], "detail": true}), &device)
        .await;
    let after_detail = usage().await;
    assert_eq!(after_detail["shown"], 2);
    assert_eq!(after_detail["fetched"], 3);
    assert_eq!(after_detail["detail_opened"], 1);
    assert!(after_detail["last_used_at"].is_number());
    app.finish().await;
}

#[tokio::test]
async fn brief_and_concurrent_topics_share_a_trusted_eight_card_budget() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("push-budget").await;
    let device = app.device(&admin, "laptop").await;
    let project = app
        .post(
            "projects/init",
            json!({"repository": "example.test/e2e/push", "name": "E2E Push budget"}),
            &device,
        )
        .await;
    let project_id = project["project_id"].as_str().unwrap();
    let shared = save(&app, &device, json!({
        "kind": "rule", "scope": "project", "title": "Shared lookup rule",
        "summary": "Set the timeout before the request.", "reason": "Every request has a deadline.",
        "context": {"project_id": project_id, "files": ["src/shared.rs"]}
    })).await;
    for (label, rank, wrong) in [
        ("Candidate rule", "candidate", 0),
        ("Doubted rule", "normal", 1),
    ] {
        let id = save(&app, &device, json!({
            "kind": "rule", "scope": "project", "title": label,
            "summary": "Use the service request counters.", "reason": "Counters help with incident triage.",
            "context": {"project_id": project_id, "files": ["src/shared.rs"]}
        })).await;
        sqlx::query("UPDATE memories SET rank = $2 WHERE memory_id = $1")
            .bind(&id)
            .bind(rank)
            .execute(app.db.store.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE usage SET wrong = $2 WHERE memory_id = $1")
            .bind(&id)
            .bind(wrong)
            .execute(app.db.store.pool())
            .await
            .unwrap();
    }
    let brief_body = json!({"session_id": "shared", "context": {"project_id": project_id}});
    let brief = app.post("session/brief", brief_body.clone(), &device).await;
    let text = brief["brief"].as_str().unwrap();
    assert!(
        text.contains(&shared) && text.contains(r#""why":"project rule""#),
        "{brief}"
    );
    for label in ["Candidate rule", "Doubted rule"] {
        assert!(!text.contains(label), "{brief}");
    }
    assert_eq!(
        app.post("session/brief", brief_body, &device).await["brief"],
        ""
    );
    let empty = app
        .post(
            "session/topic",
            json!({"session_id": "shared", "path": "src/shared.rs"}),
            &device,
        )
        .await;
    assert_eq!(empty, json!({"notes": []}));

    for i in 0..9 {
        let path = format!("src/probe{i}.rs");
        let title = format!("Working timer probe {i}");
        save(
            &app,
            &device,
            json!({
                "kind": "fact", "scope": "personal", "title": title,
                "summary": format!("Probe {i} measures how long the worker stays active."),
                "reason": "Dedicated measurements cover a different path per file.",
                "context": {"files": [path]}
            }),
        )
        .await;
    }
    // The fixtures differ by path and index. Promote them so duplicate coaching does not
    // influence the cap assertion.
    sqlx::query("UPDATE memories SET rank = 'normal' WHERE kind = 'fact'")
        .execute(app.db.store.pool())
        .await
        .unwrap();
    let app = std::sync::Arc::new(app);
    let mut requests = tokio::task::JoinSet::new();
    for i in 0..9 {
        let (app, device) = (app.clone(), device.clone());
        requests.spawn(async move {
            app.post(
                "session/topic",
                json!({
                    "session_id": "shared", "path": format!("src/probe{i}.rs")
                }),
                &device,
            )
            .await
        });
    }
    let replies = requests.join_all().await;
    let shown: Vec<_> = replies
        .iter()
        .flat_map(|reply| reply["notes"].as_array().unwrap())
        .collect();
    assert_eq!(shown.len(), 8, "{replies:?}");
    assert!(
        shown
            .iter()
            .all(|note| note["why"].as_str().unwrap().starts_with("file src/probe"))
    );
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM session_shown WHERE session_id = 'shared'")
            .fetch_one(app.db.store.pool())
            .await
            .unwrap();
    assert_eq!(remaining, 9);
    let brief_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM session_shown WHERE session_id = 'shared' AND source = 'brief'",
    )
    .fetch_one(app.db.store.pool())
    .await
    .unwrap();
    assert_eq!(brief_rows, 1);
    let exhausted = app
        .post(
            "session/topic",
            json!({"session_id":"shared", "path":"src/probe8.rs"}),
            &device,
        )
        .await;
    assert_eq!(exhausted, json!({"notes": []}));
    let prompt = app
        .post(
            "session/prompt-recall",
            json!({
                "session_id":"shared", "prompt":"Please check the working timer probe"
            }),
            &device,
        )
        .await;
    assert_eq!(prompt, json!({"text":"", "budget_spent":true}));
    let search = app
        .post("search", json!({"query":"Working timer probe 8"}), &device)
        .await;
    assert!(!ids(&search).is_empty());
    let fresh = app
        .post(
            "session/topic",
            json!({"session_id":"fresh", "path":"src/shared.rs"}),
            &device,
        )
        .await;
    assert_eq!(fresh["notes"].as_array().unwrap().len(), 1, "{fresh}");
    assert_eq!(fresh["notes"][0]["id"], shared);
    std::sync::Arc::into_inner(app).unwrap().finish().await;
}

#[tokio::test]
async fn brief_ranks_proven_ahead_of_recency_and_recent_file_matches() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "laptop").await;
    let project = app
        .post(
            "projects/init",
            json!({"repository": "r", "name": "E2E Brief ordering"}),
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
    sqlx::query("UPDATE memories SET proof_count = 3 WHERE title = 'Queue lock order'")
        .execute(app.db.store.pool())
        .await
        .unwrap();
    let brief = |session_id: &'static str, recent: serde_json::Value| {
        let (app, device) = (&app, &device);
        async move {
            let reply = app
                .post(
                    "session/brief",
                    json!({"session_id": session_id, "context": {"project_id": pid}, "recent_files": recent}),
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
    for (session, file) in [("queues", "src/jobs/worker.rs"), ("docs", "docs/build.md")] {
        let text = brief(session, json!([file])).await;
        assert!(position(&text, "Queue lock order") < position(&text, "Docs build flag"));
    }
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
                   "reason": "The review identified this as a concurrency invariant.", "context": context("main")}),
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

#[tokio::test]
async fn prompt_recall_learns_per_person_across_push_sources() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("push-owner").await;
    let device = app.device(&admin, "push-device").await;
    let member = app.member(&admin, "push-member").await;
    let other = app.device(&member, "member-device").await;
    let project = app
        .post(
            "projects/init",
            json!({
                "repository": "local:e2e-push", "name": "E2E push usage"
            }),
            &device,
        )
        .await["project_id"]
        .clone();
    let context = json!({"project_id": project, "files": ["src/push.rs"]});
    let id = save(
        &app,
        &device,
        json!({
            "kind": "preference", "scope": "personal", "title": "Sol means Codex on high effort",
            "summary": "Launch the worker on high effort when asked to work with Sol.",
            "reason": "The coordinator specifies this worker by its short name.",
            "context": context
        }),
    )
    .await;
    let project_rule = save(
        &app,
        &device,
        json!({
            "kind": "rule", "scope": "project", "title": "Preserve Quartz scheduler isolation",
            "summary": "Preserve quartz isolation during all scheduler tests.",
            "reason": "Quartz shares the scheduler lock with other fixtures.",
            "context": context
        }),
    )
    .await;
    app.post(
        "projects/init",
        json!({"repository": "local:e2e-push", "project_id": project}),
        &other,
    )
    .await;
    let prompt = |session: &str, text: &str| {
        json!({
            "session_id": session, "prompt": text, "context": {"project_id": project}
        })
    };
    let query = "Can we proceed with sol?";
    let run = |session, text| app.post("session/prompt-recall", prompt(session, text), &device);
    for skip in [
        "with sol",
        "/go ahead with sol",
        "yes please go ahead",
        "Should I paint a sunset?",
    ] {
        assert_eq!(run("skip", skip).await["text"], "");
    }
    let (first, concurrent) = tokio::join!(run("push-a", query), run("push-a", query));
    let nonempty = |reply: &serde_json::Value| !reply["text"].as_str().unwrap().is_empty();
    assert_ne!(nonempty(&first), nonempty(&concurrent));
    for session in ["push-b", "push-c"] {
        let reply = run(session, query).await;
        let text = reply["text"].as_str().unwrap();
        assert!(
            text.contains(r#"why: matches "sol" in your prompt"#),
            "{reply}"
        );
        assert!(text.len() <= 300);
        assert_eq!(run(session, query).await["text"], "");
    }
    assert_eq!(run("muted", query).await["text"], "");
    let brief = app
        .post(
            "session/brief",
            json!({"session_id":"muted", "context": context}),
            &device,
        )
        .await;
    assert!(
        !brief["brief"].as_str().unwrap().contains("Sol means"),
        "{brief}"
    );
    let topic = app
        .post(
            "session/topic",
            json!({
                "session_id":"muted", "path":"src/push.rs", "context": context
            }),
            &device,
        )
        .await;
    assert!(!topic.to_string().contains("Sol means"), "{topic}");
    let before = app.get(&format!("memories/{id}"), &admin).await;
    assert_eq!(before["usage"]["pushed"], 3);
    assert_eq!(before["usage"]["push_used"], 0);
    let records: i64 = sqlx::query_scalar("SELECT count(*) FROM lookup_log")
        .fetch_one(app.db.store.pool())
        .await
        .unwrap();
    let observations: i64 = sqlx::query_scalar("SELECT count(*) FROM observations")
        .fetch_one(app.db.store.pool())
        .await
        .unwrap();
    assert_eq!((records, observations), (0, 0));

    let search = app
        .post(
            "search",
            json!({"query":"Sol means Codex on high effort"}),
            &device,
        )
        .await;
    assert_eq!(ids(&search), std::slice::from_ref(&id));
    assert!(
        run("revived", query).await["text"]
            .as_str()
            .unwrap()
            .contains("Sol means")
    );
    let get = json!({"ids":[id], "session_id":"revived"});
    for _ in 0..2 {
        app.post("get", get.clone(), &device).await;
    }
    let view = app.get(&format!("memories/{id}"), &admin).await;
    assert_eq!(view["usage"]["pushed"], 4);
    // One credit for the search hit and one for the session's gets (repeats count once).
    assert_eq!(view["usage"]["push_used"], 2);
    assert_eq!(view["trust_level"], before["trust_level"]);

    let rule_query = "Please preserve quartz isolation";
    assert!(
        !run("vote", rule_query).await["text"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    app.post(
        "session/lookup",
        json!({"session_id":"vote", "kind":"feedback", "used_ids":[project_rule]}),
        &device,
    )
    .await;
    app.post(
        "feedback",
        json!({"id":project_rule, "vote":"helpful", "session_id":"vote"}),
        &device,
    )
    .await;
    let shared = format!("memories/{project_rule}");
    assert_eq!(app.get(&shared, &admin).await["usage"]["push_used"], 1);
    assert_eq!(app.get(&shared, &member).await["usage"]["push_used"], 0);
    assert_eq!(
        app.post(
            "session/prompt-recall",
            prompt("other-person", query),
            &other
        )
        .await["text"],
        ""
    );
    assert!(
        !app.post(
            "session/prompt-recall",
            prompt("other-person", rule_query),
            &other
        )
        .await["text"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    app.finish().await;
}

#[tokio::test]
async fn prompt_recall_narrows_candidates_before_the_hybrid_limit() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("prompt-scope").await;
    let device = app.device(&admin, "prompt-scope").await;
    let project = |name| {
        app.post(
            "projects/init",
            json!({"repository": format!("local:e2e-{name}"), "name": format!("E2E {name}")}),
            &device,
        )
    };
    let here = project("current").await["project_id"].clone();
    let elsewhere = project("other").await["project_id"].clone();
    let save_one = |scope, kind, title, project| {
        save(
            &app,
            &device,
            json!({
                "scope": scope, "kind": kind, "title": title, "context":{"project_id":project},
                "summary": "Keep opal scheduler isolation for this build path.",
                "reason": "The scheduler shares its queue with a second service."
            }),
        )
    };
    save_one(
        "personal",
        "rule",
        "Retain opal scheduler isolation",
        serde_json::Value::Null,
    )
    .await;
    save_one(
        "project",
        "procedure",
        "Keep opal scheduler isolation",
        here.clone(),
    )
    .await;
    let candidate = save_one(
        "project",
        "rule",
        "Candidate scheduler isolation",
        here.clone(),
    )
    .await;
    let doubted = save_one(
        "project",
        "rule",
        "Doubted opal scheduler isolation",
        here.clone(),
    )
    .await;
    save_one(
        "project",
        "rule",
        "Other opal scheduler isolation",
        elsewhere,
    )
    .await;
    save_one(
        "global",
        "rule",
        "Global opal scheduler isolation",
        serde_json::Value::Null,
    )
    .await;
    save_one(
        "personal",
        "fact",
        "Fact about opal scheduler isolation",
        serde_json::Value::Null,
    )
    .await;
    sqlx::query(
        "UPDATE memories SET rank = CASE WHEN memory_id = $1 THEN 'candidate' ELSE 'normal' END",
    )
    .bind(&candidate)
    .execute(app.db.store.pool())
    .await
    .unwrap();
    app.post("feedback", json!({"id":doubted, "vote":"wrong"}), &device)
        .await;
    let payload = json!({"session_id":"scope", "context":{"project_id":here}, "prompt":"Please keep opal scheduler isolation"});
    let reply = app
        .post("session/prompt-recall", payload.clone(), &device)
        .await;
    let text = reply["text"].as_str().unwrap();
    assert!(
        text.contains("Retain opal") && text.contains("Keep opal"),
        "{reply}"
    );
    assert_eq!(text.matches("(why:").count(), 2);
    assert!(text.len() <= 300);
    assert_eq!(
        app.post("session/prompt-recall", payload, &device).await["text"],
        ""
    );
    app.finish().await;
}

#[tokio::test]
async fn pins_are_web_only_and_both_brief_layers_deduplicate_automatic_recall() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("layer-owner").await;
    let device = app.device(&admin, "laptop").await;
    let outsider = app.member(&admin, "layer-outsider").await;
    let project = app
        .post(
            "projects/init",
            json!({
                "repository": "local:e2e-brief-layers", "name": "E2E Brief layers"
            }),
            &device,
        )
        .await;
    let pid = project["project_id"].as_str().unwrap();
    let context = json!({"project_id": pid, "branch": "main", "files": ["src/layers.rs"]});
    let save_rule = |title| {
        save(
            &app,
            &device,
            json!({
                "kind": "rule", "scope": "project", "title": title,
                "summary": format!("{title} is part of the startup test fixture."),
                "reason": "Startup context should include existing policies.",
                "context": context
            }),
        )
    };
    let pin = save_rule("Startup identity").await;
    let essential = save_rule("Startup policy").await;
    let superseded = save_rule("Previous policy").await;
    let forgotten = save_rule("Retired policy").await;
    save(
        &app,
        &device,
        json!({
            "kind": "rule", "scope": "project", "title": "Feature policy",
            "summary": "Set the retry behavior on the feature branch.",
            "reason": "This policy only applies on another branch.",
            "context": {"project_id": pid, "branch": "feature", "files": ["src/feature.rs"]}
        }),
    )
    .await;
    let target = format!("memories/{pin}/pin");
    for (token, expected) in [(&device, 401), (&outsider, 404)] {
        assert_eq!(
            app.send("POST", &target, json!({"pinned": true}), Some(token))
                .await
                .0,
            expected
        );
    }
    for id in [&pin, &superseded, &forgotten] {
        let pinned = app
            .post(
                &format!("memories/{id}/pin"),
                json!({"pinned": true}),
                &admin,
            )
            .await;
        assert!(pinned["pinned_at"].is_i64(), "{pinned}");
    }
    let fetched = app
        .send("GET", &format!("memories/{pin}"), json!(null), Some(&admin))
        .await;
    assert_eq!(fetched.0, 200, "{fetched:?}");
    assert!(fetched.1["pinned_at"].is_i64(), "{fetched:?}");
    app.post("forget", json!({"id": forgotten}), &admin).await;
    sqlx::query("UPDATE memories SET status = 'superseded' WHERE memory_id = $1")
        .bind(&superseded)
        .execute(app.db.store.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE memories SET rank = 'normal', created_at = now() - interval '3 days 12 hours' WHERE project_id = $1")
        .bind(pid).execute(app.db.store.pool()).await.unwrap();
    sqlx::query(
        "INSERT INTO push_usage (person_id, memory_id, pushed, muted)
                 SELECT owner_id, memory_id, 3, true FROM memories WHERE memory_id = $1",
    )
    .bind(&pin)
    .execute(app.db.store.pool())
    .await
    .unwrap();

    let brief = app
        .post(
            "session/brief",
            json!({"session_id": "layers", "context": context}),
            &device,
        )
        .await;
    let text = brief["brief"].as_str().unwrap();
    let data: serde_json::Value = serde_json::from_str(
        text.split("```")
            .nth(1)
            .unwrap()
            .strip_prefix("json\n")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(data["L0 Identity"][0]["id"], pin);
    assert_eq!(data["L0 Identity"][0]["age"], "3d");
    assert_eq!(data["L1 Essential"][0]["id"], essential);
    assert_eq!(data["L0 Identity"].as_array().unwrap().len(), 1, "{text}");
    assert_eq!(data["L1 Essential"].as_array().unwrap().len(), 1, "{text}");
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT memory_id FROM session_shown WHERE session_id = 'layers' AND source = 'brief'",
    )
    .fetch_all(app.db.store.pool())
    .await
    .unwrap();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&pin) && ids.contains(&essential));
    let topic = |session| {
        app.post(
            "session/topic",
            json!({
                "session_id": session, "context": context, "path": "src/layers.rs"
            }),
            &device,
        )
    };
    let same_session = topic("layers").await;
    let another_session = topic("fresh-layers").await;
    assert!(
        !same_session.to_string().contains(&pin) && !same_session.to_string().contains(&essential)
    );
    assert!(
        another_session.to_string().contains("Startup identity"),
        "{another_session}"
    );

    let person: String = sqlx::query_scalar("SELECT owner_id FROM projects WHERE project_id = $1")
        .bind(pid)
        .fetch_one(app.db.store.pool())
        .await
        .unwrap();
    app.db
        .store
        .set_project_archived(&person, true, pid, true)
        .await
        .unwrap();
    let archived = app
        .post(
            "session/brief",
            json!({"session_id": "archived-layers", "context": context}),
            &device,
        )
        .await;
    assert_eq!(archived["brief"], "");
    app.db
        .store
        .set_project_archived(&person, true, pid, false)
        .await
        .unwrap();
    assert_eq!(
        app.post(&target, json!({"pinned": false}), &admin).await["pinned_at"],
        json!(null)
    );
    let unpinned = app
        .post(
            "session/brief",
            json!({"session_id": "unpinned-layers", "context": context}),
            &device,
        )
        .await;
    assert!(
        !unpinned["brief"].as_str().unwrap().contains(&pin),
        "{unpinned}"
    );
    app.finish().await;
}

#[tokio::test]
async fn legacy_brief_keeps_identity_and_packs_essentials_within_1200_bytes() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("brief-compat").await;
    let device = app.device(&admin, "laptop").await;
    let project = app
        .post(
            "projects/init",
            json!({"repository": "local:e2e-brief-compat", "name": "E2E Brief compatibility"}),
            &device,
        )
        .await;
    let project_id = project["project_id"].as_str().unwrap();
    sqlx::query(
        "INSERT INTO memories (memory_id, owner_id, project_id, scope, kind, title, summary,
                               evidence_hash, identity_hash, pinned_at)
         SELECT 'brief_' || n, p.owner_id, p.project_id, 'project', 'rule', 'Brief card ' || n,
                repeat('é', 120), 'e', 'brief_' || n, CASE WHEN n <= 2 THEN now() END
           FROM projects p CROSS JOIN generate_series(1, 8) n WHERE p.project_id = $1",
    )
    .bind(project_id)
    .execute(app.db.store.pool())
    .await
    .unwrap();

    let body = json!({"session_id": "legacy-brief", "context": {"project_id": project_id}});
    let legacy = app.post("session/brief", body.clone(), &device).await;
    let mut extended_body = body;
    extended_body["session_id"] = json!("extended-brief");
    extended_body["extended_brief"] = json!(true);
    let extended = app.post("session/brief", extended_body, &device).await;
    assert!(extended["brief"].as_str().unwrap().len() > 1200);

    let decode = |reply: &serde_json::Value, max_bytes| -> serde_json::Value {
        assert_eq!(reply["max_bytes"], max_bytes);
        let text = reply["brief"].as_str().unwrap();
        assert!(text.len() <= max_bytes, "{text}");
        serde_json::from_str(
            text.split_once("```json\n")
                .unwrap()
                .1
                .strip_suffix("\n```")
                .unwrap(),
        )
        .unwrap()
    };
    let legacy = decode(&legacy, 1200);
    let extended = decode(&extended, 1800);
    let legacy_l0_ids: Vec<_> = legacy["L0 Identity"]
        .as_array()
        .unwrap()
        .iter()
        .map(|card| card["id"].as_str().unwrap())
        .collect();
    let extended_l0_ids: Vec<_> = extended["L0 Identity"]
        .as_array()
        .unwrap()
        .iter()
        .map(|card| card["id"].as_str().unwrap())
        .collect();
    assert_eq!(legacy_l0_ids, vec!["brief_1", "brief_2"]);
    assert_eq!(extended_l0_ids, vec!["brief_1", "brief_2"]);
    let legacy_essentials = legacy["L1 Essential"].as_array().unwrap();
    let extended_essentials = extended["L1 Essential"].as_array().unwrap();
    assert!(!legacy_essentials.is_empty() && legacy_essentials.len() < extended_essentials.len());

    let shown: Vec<String> = sqlx::query_scalar(
        "SELECT memory_id FROM session_shown WHERE session_id = 'legacy-brief' ORDER BY memory_id",
    )
    .fetch_all(app.db.store.pool())
    .await
    .unwrap();
    let mut rendered: Vec<&str> = legacy["L0 Identity"]
        .as_array()
        .unwrap()
        .iter()
        .chain(legacy_essentials)
        .map(|card| card["id"].as_str().unwrap())
        .collect();
    rendered.sort_unstable();
    assert_eq!(shown, rendered);
    app.finish().await;
}

#[tokio::test]
async fn hermes_chat_recall_keeps_personal_memories_in_their_chat() {
    let Some(app) = App::new().await else { return };
    let owner = app.admin("chat-owner").await;
    let token = app.device(&owner, "hermes-gateway").await;
    let other = app.member(&owner, "other-account").await;
    let foreign_token = app.device(&other, "other-device").await;
    let project = app
        .post(
            "projects/init",
            json!({
                "repository": "local:e2e-hermes-chat", "name": "E2E Hermes chat"
            }),
            &token,
        )
        .await["project_id"]
        .clone();
    let chat = |kind, hash: &str| {
        json!({
            "platform": "matrix", "kind": kind, "chat_id_hash": hash.repeat(64),
            "thread_id_hash": "d".repeat(64),
        })
    };
    let group = chat("group", "a");
    let other_group = chat("group", "b");
    let dm = chat("dm", "c");
    let private = save(
        &app,
        &token,
        json!({
            "kind": "preference", "title": "Kestrel private fallback",
            "summary": "Use the private Kestrel fallback when generating my reports.",
            "reason": "The fallback preserves the account's report format."
        }),
    )
    .await;
    let pinned = save(
        &app,
        &token,
        json!({
            "kind": "note", "title": "Private identity pin",
            "summary": "This pin is available only to the account owner."
        }),
    )
    .await;
    app.post(
        &format!("memories/{pinned}/pin"),
        json!({"pinned": true}),
        &owner,
    )
    .await;
    let project_rule = save(
        &app,
        &token,
        json!({
            "kind": "rule", "scope": "project", "title": "Kestrel project schedule",
            "summary": "Refresh the Kestrel schedule on the first day of the month.",
            "reason": "Shared reports read the monthly schedule.",
            "context": {"project_id": project}
        }),
    )
    .await;
    let saved_body = json!({
        "kind": "note", "scope": "global", "title": "Kestrel observatory schedule",
        "summary": "The Kestrel observatory runs a public tour every Thursday.",
        "context": {"chat": group}
    });
    let group_id = save(&app, &token, saved_body.clone()).await;
    assert_eq!(save(&app, &token, saved_body.clone()).await, group_id);
    let record = app.get(&format!("memories/{group_id}"), &owner).await;
    assert_eq!(record["scope"], "personal");
    assert_eq!(record["trust"], "new · awaiting confirmation");
    assert_eq!(record["project_id"], serde_json::Value::Null);
    let stored: (String, i32) =
        sqlx::query_as("SELECT chat_id_hash, proof_count FROM memories WHERE memory_id = $1")
            .bind(&group_id)
            .fetch_one(app.db.store.pool())
            .await
            .unwrap();
    assert_eq!(stored, ("a".repeat(64), 1));

    let search = async |chat: &serde_json::Value, token: &str| {
        let args = json!({"query": "Kestrel", "context": {"chat": chat}});
        ids(&app.post("search", args, token).await)
    };
    assert_eq!(search(&group, &token).await, [group_id.as_str()]);
    assert!(search(&other_group, &token).await.is_empty());
    assert!(search(&dm, &token).await.is_empty());
    let dm_saved = save(&app, &token, json!({
        "kind": "note", "title": "Kestrel ticket", "summary": "The next Kestrel tour runs on Thursday.",
        "context": {"chat": dm}
    })).await;
    assert_eq!(search(&dm, &token).await, [dm_saved.as_str()]);
    assert_eq!(search(&group, &token).await, [group_id.as_str()]);
    assert!(search(&group, &foreign_token).await.is_empty());

    let project_save = json!({
        "kind": "note", "title": "Kestrel tour capacity",
        "summary": "The Kestrel tour can take twelve passengers.",
        "context": {"chat": group, "project_id": "E2E Hermes chat"}
    });
    let project_note = save(&app, &token, project_save.clone()).await;
    let record = app.get(&format!("memories/{project_note}"), &owner).await;
    assert_eq!(record["scope"], "project");
    assert_eq!(record["project_id"], project);
    assert_eq!(record["trust"], "new · awaiting confirmation");
    let tag: Option<String> =
        sqlx::query_scalar("SELECT chat_id_hash FROM memories WHERE memory_id = $1")
            .bind(&project_note)
            .fetch_one(app.db.store.pool())
            .await
            .unwrap();
    assert_eq!(tag, None);
    let denied_search = json!({
        "query": "Kestrel", "context": {"chat": group, "project_id": project}
    });
    for (route, body) in [
        ("search", denied_search),
        ("remember", project_save.clone()),
    ] {
        assert_eq!(
            app.send("POST", route, body, Some(&foreign_token)).await.0,
            404
        );
    }
    let mut preference = project_save.clone();
    preference["kind"] = "preference".into();
    let (status, reply) = app.send("POST", "remember", preference, Some(&token)).await;
    assert_eq!(status, 400);
    assert_eq!(
        reply["error"],
        cairn_core::process::prepare::CHAT_PREFERENCE_ERROR
    );

    let selected = app
        .post(
            "search",
            json!({"query": "Kestrel", "context": {
                "chat": group, "project_id": "E2E Hermes chat"
            }}),
            &token,
        )
        .await;
    let selected = ids(&selected);
    assert!(selected.contains(&project_rule) && selected.contains(&project_note));
    assert!(selected.contains(&group_id));
    assert!(!selected.contains(&private) && !selected.contains(&dm_saved));
    assert_eq!(search(&group, &token).await, [group_id.as_str()]);
    assert!(
        !ids(&app
            .post("search", json!({"query": "Kestrel"}), &token)
            .await)
        .contains(&group_id)
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("memories/{group_id}"),
            serde_json::Value::Null,
            Some(&token)
        )
        .await
        .0,
        404
    );

    for (key, invalid) in [
        ("chat_id_hash", json!("unhashed-transport-id")),
        ("sender_id_hash", json!("e".repeat(64))),
        ("owner", json!(true)),
        ("platform", json!("Uppercase")),
    ] {
        let mut invalid_chat = saved_body.clone();
        invalid_chat["context"]["chat"][key] = invalid;
        assert_eq!(
            app.send("POST", "remember", invalid_chat, Some(&token))
                .await
                .0,
            400
        );
    }

    let session = |digit: char| format!("hchat_{}", digit.to_string().repeat(32));
    let brief = app
        .post(
            "session/brief",
            json!({
                "session_id": session('a'), "context": {"chat": group, "project_id": project}
            }),
            &token,
        )
        .await;
    let briefing = brief["brief"].as_str().unwrap();
    assert!(briefing.contains("Kestrel project schedule"), "{brief}");
    assert!(!briefing.contains("private fallback") && !briefing.contains("Private identity pin"));
    assert_eq!(
        brief["instructions"]["chat_instruction"],
        cairn_core::instructions::DEFAULT_CHAT_INSTRUCTION
    );
    let dm_brief = app
        .post(
            "session/brief",
            json!({
                "session_id": session('b'), "context": {"chat": dm}
            }),
            &token,
        )
        .await;
    assert!(
        !dm_brief["brief"]
            .as_str()
            .unwrap()
            .contains("Private identity pin")
    );
    for (chat, digit) in [(&group, 'c'), (&dm, 'd')] {
        let prompt = app
            .post(
                "session/prompt-recall",
                json!({
                    "session_id": session(digit), "prompt": "Can I use Kestrel private fallback?",
                    "context": {"chat": chat}
                }),
                &token,
            )
            .await;
        assert!(
            !prompt["text"]
                .as_str()
                .unwrap()
                .contains("Kestrel private fallback"),
            "{prompt}"
        );
    }
    save(&app, &token, json!({
        "kind": "gotcha", "title": "Kestrel command warning",
        "summary": "Check the report directory before running kestrel audit.",
        "reason": "The audit replaces the existing report.",
        "fields": {"symptom": "The previous report disappeared",
                   "cause": "kestrel audit overwrites it", "fix": "Copy the previous report first"},
        "context": {"commands": ["kestrel audit"]}
    })).await;
    for (chat, digit) in [(&group, 'e'), (&dm, 'f')] {
        let warning = app
            .post(
                "session/command-recall",
                json!({
                    "session_id": session(digit), "command": "kestrel audit", "chat": chat
                }),
                &token,
            )
            .await;
        assert!(
            !warning["text"]
                .as_str()
                .unwrap()
                .contains("Kestrel command warning"),
            "{warning}"
        );
    }
    app.db.finish().await;
}

#[tokio::test]
async fn web_pin_summary_reports_quotas_and_overflow_matching_brief() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("pin-web-owner").await;
    let outsider = app.member(&admin, "pin-web-outsider").await;
    let device = app.device(&admin, "laptop").await;
    let project = app
        .post(
            "projects/init",
            json!({"repository": "local:pin-web", "name": "Pin Web Summary"}),
            &device,
        )
        .await;
    let pid = project["project_id"].as_str().unwrap();

    // Web-only and membership checks on GET /memories/pins.
    assert_eq!(
        app.send("GET", "memories/pins", json!(null), Some(&device))
            .await
            .0,
        401
    );
    assert_eq!(
        app.send(
            "GET",
            &format!("memories/pins?projectId={pid}"),
            json!(null),
            Some(&outsider)
        )
        .await
        .0,
        404
    );

    let mut per_ids = Vec::new();
    for i in 0..4 {
        let id = save(
            &app,
            &device,
            json!({
                "kind": "rule", "scope": "personal",
                "title": format!("Personal rule number {i}"),
                "summary": "Personal pin summary long enough to consume L0 bytes cleanly.".repeat(2),
                "reason": "Testing personal pin quota and L0 fit",
            }),
        )
        .await;
        per_ids.push(id);
    }
    for id in &per_ids[..3] {
        app.post(
            &format!("memories/{id}/pin"),
            json!({"pinned": true}),
            &admin,
        )
        .await;
    }
    let (status, _) = app
        .send(
            "POST",
            &format!("memories/{}/pin", per_ids[3]),
            json!({"pinned": true}),
            Some(&admin),
        )
        .await;
    assert_eq!(status, 409);

    let mut prj_ids = Vec::new();
    for i in 0..5 {
        let id = save(
            &app,
            &device,
            json!({
                "kind": "rule", "scope": "project",
                "title": format!("Project rule number {i}"),
                "summary": "Project pin summary long enough to consume L0 bytes cleanly.".repeat(2),
                "reason": "Testing project pin quota and L0 fit",
                "context": {"project_id": pid},
            }),
        )
        .await;
        prj_ids.push(id);
    }
    for id in &prj_ids[..4] {
        app.post(
            &format!("memories/{id}/pin"),
            json!({"pinned": true}),
            &admin,
        )
        .await;
    }
    let (status, _) = app
        .send(
            "POST",
            &format!("memories/{}/pin", prj_ids[4]),
            json!({"pinned": true}),
            Some(&admin),
        )
        .await;
    assert_eq!(status, 409);

    let summary = app
        .send(
            "GET",
            &format!("memories/pins?projectId={pid}"),
            json!(null),
            Some(&admin),
        )
        .await;
    assert_eq!(summary.0, 200, "{summary:?}");
    let summary = summary.1;
    assert_eq!(summary["personal_quota_used"], 3);
    assert_eq!(summary["personal_quota_limit"], 3);
    assert_eq!(summary["project_quota_used"], 4);
    assert_eq!(summary["project_quota_limit"], 4);
    assert_eq!(summary["total"], 7);
    let not_shown = summary["not_shown"].as_u64().unwrap();
    assert!(not_shown > 0, "{summary}");

    // Promote the unpinned personal rule so it qualifies for L1 Essential.
    sqlx::query("UPDATE memories SET rank = 'normal' WHERE memory_id = $1")
        .bind(&per_ids[3])
        .execute(app.db.store.pool())
        .await
        .unwrap();

    let brief = app
        .post(
            "session/brief",
            json!({"session_id": "pin-overflow", "extended_brief": true, "context": {"project_id": pid}}),
            &device,
        )
        .await;
    assert_eq!(brief["pins_not_shown"], not_shown, "{brief}");
    let text = brief["brief"].as_str().unwrap();
    assert!(text.contains(&format!(
        "\"L0_not_shown\":\"{not_shown} more pinned not shown\""
    )));
    // Unpinned personal rule reaches L1 with "personal rule" why-label.
    assert!(
        text.contains(&per_ids[3]) && text.contains(r#""why":"personal rule""#),
        "{text}"
    );
    app.finish().await;
}

#[tokio::test]
async fn get_field_and_timeline_stay_on_visible_memories() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("owner").await;
    let owner = app.device(&admin, "owner").await;
    let other = app
        .device(&app.member(&admin, "other").await, "other")
        .await;
    let old = save(
        &app,
        &owner,
        json!({
            "kind": "gotcha",
            "title": "Old quartz claim panic",
            "summary": "The old note.",
            "reason": "It moved the guard.",
            "fields": {"symptom": "panic on quartz_claim", "cause": "moved", "fix": "borrow"}
        }),
    )
    .await;
    let newer = save(
        &app,
        &owner,
        json!({
            "kind": "gotcha",
            "title": "Borrow the quartz claim",
            "summary": "The new note.",
            "reason": "Borrow until the update returns.",
            "fields": {"symptom": "same panic", "cause": "moved", "fix": "borrow the guard"},
            "supersedes": old
        }),
    )
    .await;
    let got = app
        .post(
            "get",
            json!({"ids": [newer], "field": "fix", "timeline": true}),
            &owner,
        )
        .await;
    assert_eq!(got["memories"][0]["fields"]["fix"], "borrow the guard");
    assert!(got["memories"][0]["fields"].get("symptom").is_none());
    assert!(got["memories"][0].get("reason").is_none());
    let chain: Vec<&str> = got["memories"][0]["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert!(chain.contains(&old.as_str()), "{got}");
    let hidden = app
        .post("get", json!({"ids": [newer], "timeline": true}), &other)
        .await;
    assert!(
        hidden["memories"].as_array().unwrap().is_empty(),
        "{hidden}"
    );
    let (status, _) = app
        .send(
            "POST",
            "get",
            json!({"ids": [newer], "field": "nope"}),
            Some(&owner),
        )
        .await;
    assert_eq!(status, 400);
    app.finish().await;
}

#[tokio::test]
async fn recorded_branch_and_version_selectors_do_not_cross_people() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("time").await;
    let owner = app.device(&admin, "time").await;
    let other = app
        .device(&app.member(&admin, "time-other").await, "time-other")
        .await;
    let january = save(
        &app,
        &owner,
        json!({
            "kind": "fact",
            "title": "January fonts",
            "summary": "Invoice fonts live in harbor-cli 1.4.2",
            "reason": "A missing font changed the layout.",
            "fields": {"source": "notes"},
            "context": {"branch": "feature/amber"}
        }),
    )
    .await;
    let june = save(
        &app,
        &owner,
        json!({
            "kind": "fact",
            "title": "June fonts",
            "summary": "Invoice fonts were copied aside.",
            "reason": "The june copy is not the january one.",
            "fields": {"source": "notes"}
        }),
    )
    .await;
    let hidden = save(
        &app,
        &other,
        json!({
            "kind": "fact",
            "title": "January fonts",
            "summary": "Invoice fonts live in harbor-cli 1.4.2",
            "reason": "Someone else's copy.",
            "fields": {"source": "notes"}
        }),
    )
    .await;
    sqlx::query("UPDATE memories SET created_at = '2026-01-15T00:00:00Z' WHERE memory_id = $1")
        .bind(&january)
        .execute(app.state.store.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE memories SET created_at = '2026-01-15T00:00:00Z' WHERE memory_id = $1")
        .bind(&hidden)
        .execute(app.state.store.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE memories SET created_at = '2026-06-01T00:00:00Z' WHERE memory_id = $1")
        .bind(&june)
        .execute(app.state.store.pool())
        .await
        .unwrap();
    let found = app
        .post(
            "search",
            json!({
                "query": "invoice fonts",
                "recorded_from": "2026-01-15",
                "recorded_to": "2026-01-15",
                "branch": "feature/amber",
                "version": "1.4.2"
            }),
            &owner,
        )
        .await;
    assert_eq!(ids(&found), vec![january], "{found}");
    let (status, _) = app
        .send(
            "POST",
            "search",
            json!({"query": "invoice fonts", "recorded_from": "yesterday"}),
            Some(&owner),
        )
        .await;
    assert_eq!(status, 400);
    app.finish().await;
}

#[tokio::test]
async fn retrieval_trace_is_private_redacted_and_off_by_default() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("trace").await;
    let owner = app.device(&admin, "trace").await;
    let other = app.member(&admin, "trace-other").await;
    let saved = save(
        &app,
        &owner,
        json!({
            "kind": "fact",
            "title": "Trace target",
            "summary": "A visible fact for the trace.",
            "reason": "So search has something to return.",
            "fields": {"source": "notes"}
        }),
    )
    .await;
    // Off: a search writes nothing. The secret is real, but this call is not stored.
    app.post(
        "search",
        json!({"query": "trace target token=supersecretvalue"}),
        &owner,
    )
    .await;
    let empty = app.get("me/trace", &admin).await;
    assert_eq!(empty["enabled"], false);
    assert!(empty["traces"].as_array().unwrap().is_empty(), "{empty}");
    app.post("me/trace", json!({"enabled": true}), &admin).await;
    // The query repeats the memory's own words so the relevance gate keeps the hit, plus one secret.
    let found = app
        .post(
            "search",
            json!({"query": "trace target visible fact token=supersecretvalue"}),
            &owner,
        )
        .await;
    assert!(ids(&found).contains(&saved), "{found}");
    let mine = app.get("me/trace", &admin).await;
    let query = mine["traces"][0]["query"].as_str().unwrap();
    assert!(query.contains("trace target"), "{mine}");
    assert!(!query.contains("supersecretvalue"), "{mine}");
    assert!(query.contains("[REDACTED]"), "{mine}");
    let hit = &mine["traces"][0]["results"][0];
    assert_eq!(hit["id"], saved);
    assert!(hit.get("title").is_none(), "{mine}");
    assert!(hit.get("summary").is_none(), "{mine}");
    let channel = hit["channel"].as_str().unwrap();
    assert!(
        !channel.contains(' ') && !channel.contains("visible"),
        "{mine}"
    );
    assert_eq!(hit["opened"], false);
    assert_eq!(hit["voted"], false);
    app.post("get", json!({"ids": [saved]}), &owner).await;
    let opened = app.get("me/trace", &admin).await;
    assert_eq!(
        opened["traces"][0]["results"][0]["opened"], true,
        "{opened}"
    );
    assert_eq!(
        opened["traces"][0]["results"][0]["voted"], false,
        "{opened}"
    );
    app.post("feedback", json!({"id": &saved, "vote": "helpful"}), &owner)
        .await;
    let voted = app.get("me/trace", &admin).await;
    assert_eq!(voted["traces"][0]["results"][0]["voted"], true, "{voted}");
    sqlx::query("UPDATE retrieval_traces SET expires_at = now() - interval '1 second'")
        .execute(app.state.store.pool())
        .await
        .unwrap();
    let expired = app.get("me/trace", &admin).await;
    assert_eq!(expired["enabled"], true);
    assert!(
        expired["traces"].as_array().unwrap().is_empty(),
        "{expired}"
    );
    let theirs = app.get("me/trace", &other).await;
    assert!(theirs["traces"].as_array().unwrap().is_empty(), "{theirs}");
    let (status, _) = app.send("GET", "me/trace", json!(null), Some(&owner)).await;
    assert_eq!(status, 401);
    app.finish().await;
}

#[tokio::test]
async fn search_projects_require_membership_and_keep_the_current_project_first() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("projects").await;
    let owner = app.device(&admin, "projects").await;
    let other = app
        .device(&app.member(&admin, "projects-other").await, "other")
        .await;
    let amber = app.project(&owner, "amber").await;
    let wharf = app.project(&owner, "wharf").await;
    let here = save(
        &app,
        &owner,
        json!({
            "kind": "fact",
            "scope": "project",
            "title": "Lantern policy",
            "summary": "Amber keeps the lantern policy.",
            "reason": "The checkout project.",
            "fields": {"source": "notes"},
            "context": {"project_id": amber, "repository": "amber"}
        }),
    )
    .await;
    let there = save(
        &app,
        &owner,
        json!({
            "kind": "fact",
            "scope": "project",
            "title": "Lantern policy for the dock",
            "summary": "Wharf keeps the dock lantern policy.",
            "reason": "The other project.",
            "fields": {"source": "notes"},
            "context": {"project_id": wharf, "repository": "wharf"}
        }),
    )
    .await;
    let context = json!({"project_id": amber, "repository": "amber"});
    let scoped = app
        .post(
            "search",
            json!({"query": "lantern policy dock", "context": context}),
            &owner,
        )
        .await;
    assert_eq!(ids(&scoped), vec![here.clone()], "{scoped}");
    assert_eq!(scoped["results"][0]["project"], amber);
    let widened = app
        .post(
            "search",
            json!({
                "query": "lantern policy dock",
                "context": {"project_id": amber, "repository": "amber"},
                "projects": [wharf]
            }),
            &owner,
        )
        .await;
    assert_eq!(
        ids(&widened),
        vec![here.clone(), there.clone()],
        "{widened}"
    );
    assert_eq!(widened["results"][0]["project"], amber);
    assert_eq!(widened["results"][1]["project"], wharf);
    let (status, body) = app
        .send(
            "POST",
            "search",
            json!({"query": "lantern policy", "projects": [wharf]}),
            Some(&other),
        )
        .await;
    assert_eq!(status, 403, "{body}");
    assert!(!body.to_string().contains(&here), "{body}");
    let (status, _) = app
        .send(
            "POST",
            "search",
            json!({"query": "lantern", "projects": ["a", "b", "c", "d", "e", "f"]}),
            Some(&owner),
        )
        .await;
    assert_eq!(status, 400);
    app.finish().await;
}
