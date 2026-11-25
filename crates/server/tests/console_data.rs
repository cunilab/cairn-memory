//! The console's reads: memory list filters and detail, projects, branches, members, devices,
//! health, and the owned-memories export and import.

mod common;
use common::App;
use serde_json::{Value, json};

const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

fn rule(title: &str) -> Value {
    json!({"kind": "rule", "title": title, "summary": format!("{title} is the rule here."), "reason": "the test needs a reason"})
}

fn on(mut body: Value, branch: &str) -> Value {
    body["context"] = json!({"branch": branch});
    body
}

fn in_project(mut body: Value, project: &str, branch: &str, files: &[&str]) -> Value {
    body["context"] = json!({"project_id": project, "repository": "github.com/acme/widgets", "branch": branch, "files": files});
    body
}

async fn sql(app: &App, query: &str) {
    sqlx::query(query)
        .execute(app.db.store.pool())
        .await
        .unwrap();
}

fn ids(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn memory_list_filters_q_branch_and_expired() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "d").await;
    let a = app
        .save(&device, on(rule("Alpha billing rule"), "main"))
        .await;
    let b = app
        .save(&device, on(rule("Bravo gateway rule"), "feat/x"))
        .await;
    let c = app
        .save(&device, on(rule("Charlie expired rule"), "main"))
        .await;
    let d = app
        .save(&device, on(rule("Delta stale rule"), "main"))
        .await;
    sql(
        &app,
        &format!(
            "UPDATE memories SET expires_at = now() - interval '1 day' WHERE memory_id = '{c}'"
        ),
    )
    .await;
    sql(
        &app,
        &format!(
            "UPDATE memories SET status = 'stale', stale_cause = 'feedback' WHERE memory_id = '{d}'"
        ),
    )
    .await;

    assert_eq!(
        ids(&app.get("memories?q=billing", &admin).await),
        [a.as_str()]
    );
    assert_eq!(
        ids(&app.get("memories?q=%22gateway+rule%22", &admin).await),
        [b.as_str()]
    );
    assert!(ids(&app.get("memories?q=nothingmatcheslikethis", &admin).await).is_empty());
    assert_eq!(
        ids(&app.get("memories?branch=feat/x", &admin).await),
        [b.as_str()]
    );
    let (status, _) = app
        .send("GET", "memories?status=expired", json!(null), Some(&admin))
        .await;
    assert_eq!(status, 400, "expired is not a status filter");
    assert_eq!(
        ids(&app.get("memories?status=stale", &admin).await),
        [d.as_str()]
    );
    let default = ids(&app.get("memories", &admin).await);
    assert!(default.contains(&c), "an old expires_at stays in the list");
    assert_eq!(default.len(), 4);
    let (status, body) = app
        .send(
            "GET",
            "memories?status=forgotten-ish",
            json!(null),
            Some(&admin),
        )
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (400, Some("validation")),
        "{body}"
    );
    // Listing, searching and opening never count as a fetch.
    app.get(&format!("memories/{a}"), &admin).await;
    let fetched: i64 = sqlx::query_scalar("SELECT coalesce(sum(fetched), 0)::bigint FROM usage")
        .fetch_one(app.db.store.pool())
        .await
        .unwrap();
    assert_eq!(fetched, 0);
    // Rows carry what the table shows.
    let row = &app.get("memories?status=stale", &admin).await[0];
    assert_eq!(
        (row["stale_cause"].as_str(), row["fetched"].as_i64()),
        (Some("feedback"), Some(0))
    );
    assert!(row.get("project_name").is_some());
    app.finish().await;
}

#[tokio::test]
async fn memory_detail_has_origin_usage_lineage_and_why_stale() {
    let Some(app) = App::new().await else { return };
    let alice = app.admin("alice").await;
    let bob = app.member(&alice, "bob").await;
    let (alice_d, bob_d) = (
        app.device(&alice, "laptop-claude").await,
        app.device(&bob, "bob-box").await,
    );
    let project = app.project(&alice_d, "github.com/acme/widgets").await;
    app.post(
        "projects/init",
        json!({"project_id": project, "repository": "x"}),
        &bob_d,
    )
    .await;
    let old = app
        .save(
            &alice_d,
            in_project(rule("Money is float dollars"), &project, "main", &[]),
        )
        .await;
    let cites = app
        .save(
            &alice_d,
            in_project(
                rule("Totals live in a module"),
                &project,
                "main",
                &["src/gone.rs"],
            ),
        )
        .await;
    let mut newer = in_project(rule("Money is integer cents"), &project, "main", &[]);
    newer["supersedes"] = json!(old);
    let new = app.save(&alice_d, newer).await;
    app.post("get", json!({"ids": [new]}), &alice_d).await;

    let view = app.get(&format!("memories/{new}"), &alice).await;
    assert_eq!(view["origin"], "agent");
    assert_eq!(view["project_name"], "widgets");
    assert_eq!(view["owner_is_me"], true);
    assert_eq!(view["device_name"], "laptop-claude");
    assert_eq!(view["usage"]["fetched"], 1);
    assert_eq!(view["usage"]["helpful"], 0);
    assert!(view["usage"]["last_used_at"].is_i64());
    assert!(view["status_changed_at"].is_i64() && view["created_at"].is_i64());
    assert!(view["checked_at"].is_null());
    assert_eq!(view["missing_entities"], json!([]));
    let link = &view["links"][0];
    assert_eq!(
        (link["relation"].as_str(), link["direction"].as_str()),
        (Some("supersedes"), Some("out"))
    );
    assert_eq!(
        link["other"],
        json!({"id": old, "kind": "rule", "title": "Money is float dollars", "status": "superseded"})
    );
    // The superseded memory sees the same link coming in.
    let incoming = app.get(&format!("memories/{old}"), &alice).await;
    assert_eq!(incoming["links"][0]["direction"], "in");
    assert_eq!(
        incoming["links"][0]["other"]["title"],
        "Money is integer cents"
    );
    // Opening it did not count another fetch.
    assert_eq!(
        app.get(&format!("memories/{new}"), &alice).await["usage"]["fetched"],
        1
    );

    // A project member sees it, but not whose device wrote it.
    let theirs = app.get(&format!("memories/{new}"), &bob).await;
    assert_eq!(theirs["owner_is_me"], false);
    assert!(theirs["device_name"].is_null());

    // "Why stale": the cited file is gone on the default branch.
    app.post(
        "session/repo-state",
        json!({"project_id": project, "branch": "main", "files_missing": ["src/gone.rs"]}),
        &alice_d,
    )
    .await;
    let stale = app.get(&format!("memories/{cites}"), &alice).await;
    assert_eq!(stale["stale_cause"], "missing");
    assert_eq!(stale["missing_entities"], json!(["src/gone.rs"]));
    assert!(stale["checked_at"].is_i64());
    // Agents keep their narrow view: `/get` shows no usage, owner or project name.
    let record = &app.post("get", json!({"ids": [new]}), &alice_d).await["memories"][0];
    for key in [
        "usage",
        "owner",
        "project_name",
        "device_name",
        "origin",
        "checked_at",
    ] {
        assert!(record.get(key).is_none(), "/get must not show {key}");
    }
    app.finish().await;
}

#[tokio::test]
async fn project_list_counts_branch_handoffs_and_member_activity() {
    let Some(app) = App::new().await else { return };
    let alice = app.admin("alice").await;
    let bob = app.member(&alice, "bob").await;
    let carol = app.member(&alice, "carol").await;
    let (alice_d, bob_d, carol_d) = (
        app.device(&alice, "laptop-claude").await,
        app.device(&bob, "b").await,
        app.device(&carol, "c").await,
    );
    let project = app.project(&alice_d, "github.com/acme/widgets").await;
    let empty = app.project(&alice_d, "github.com/acme/empty").await;
    for device in [&bob_d, &carol_d] {
        app.post(
            "projects/init",
            json!({"project_id": project, "repository": "x"}),
            device,
        )
        .await;
    }
    let one = app
        .save(
            &alice_d,
            in_project(rule("Widget rule one"), &project, "main", &[]),
        )
        .await;
    app.save(
        &alice_d,
        in_project(rule("Widget rule two"), &project, "main", &[]),
    )
    .await;
    app.save(
        &bob_d,
        in_project(rule("Widget rule three"), &project, "feat/x", &[]),
    )
    .await;
    sql(&app, &format!("UPDATE memories SET status = 'stale', stale_cause = 'missing' WHERE memory_id = '{one}'")).await;

    let list = app.get("projects", &alice).await;
    let entry = |id: &str| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|p| p["project_id"] == id)
            .unwrap()
            .clone()
    };
    let widgets = entry(&project);
    assert_eq!(widgets["memories"], 3, "{widgets}");
    assert_eq!(widgets["stale"], 1);
    assert_eq!(widgets["members"], 3);
    assert!(widgets["last_activity_at"].is_i64());
    assert!(widgets.get("default_branch").is_some());
    let none = entry(&empty);
    assert_eq!(
        (
            none["memories"].as_i64(),
            none["stale"].as_i64(),
            none["members"].as_i64()
        ),
        (Some(0), Some(0), Some(1))
    );
    assert!(none["last_activity_at"].is_null());

    // Members: last activity is the latest session or save in the project.
    let members = app
        .get(&format!("projects/{project}/members"), &alice)
        .await;
    let active = |email: &str| {
        members
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["email"] == email)
            .unwrap()["last_active_at"]
            .clone()
    };
    assert!(active("alice@example.test").is_i64());
    assert!(active("bob@example.test").is_i64());
    assert!(
        active("carol@example.test").is_null(),
        "carol never did anything here"
    );

    // Branches: the latest visible handoff per branch.
    let context = json!({"project_id": project, "repository": "github.com/acme/widgets", "branch": "feat/refunds", "files": ["src/refund.rs"]});
    for (id, kind, content) in [
        (
            "e1",
            "assistant",
            "Decision: refund through the ledger\nReason: one source of truth",
        ),
        ("e2", "prompt", "TODO: cover partial refunds"),
    ] {
        app.post("observe", json!({"event_id": id, "session_id": "s1", "kind": kind, "content": content, "context": context}), &alice_d).await;
    }
    app.post("session/end", json!({"session_id": "s1"}), &alice_d)
        .await;
    app.settle().await;
    let branches = app
        .get(&format!("projects/{project}/branches"), &alice)
        .await;
    let by = |name: &str| {
        branches
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["branch"] == name)
            .unwrap_or_else(|| panic!("{name}: {branches}"))
            .clone()
    };
    let refunds = by("feat/refunds");
    let handoff = &refunds["handoff"];
    assert!(
        handoff["id"].is_string() && handoff["title"].is_string() && handoff["summary"].is_string(),
        "{refunds}"
    );
    assert!(handoff["fields"].is_object() && handoff["updated_at"].is_i64());
    assert_eq!(handoff["device_name"], "laptop-claude");
    assert!(by("main")["handoff"].is_null());
    // The project's memory list carries the same console fields as the global list.
    let listed = app
        .get(&format!("projects/{project}/memories"), &alice)
        .await;
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["project_name"] == "widgets"
                && m["usage"].is_object()
                && m["links"].is_array())
    );
    // Another member sees the same project handoff only if it is visible to them.
    let theirs = app.get(&format!("projects/{project}/branches"), &bob).await;
    assert!(
        theirs
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["branch"] == "feat/x")
    );
    app.finish().await;
}

#[tokio::test]
async fn devices_report_client_health() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let mut tokens = Vec::new();
    for name in ["never", "working", "old", "legacy", "quiet", "mcp"] {
        tokens.push((name, app.device(&admin, name).await));
    }
    let token = |name: &str| tokens.iter().find(|(n, _)| *n == name).unwrap().1.clone();
    let hook = |name: &str, agent: &str| {
        let (token, ua) = (token(name), agent.to_owned());
        let app = &app;
        async move {
            app.send_with(
                "POST",
                "session/brief",
                json!({"session_id": "s"}),
                Some(&token),
                &[("user-agent", &ua)],
            )
            .await;
        }
    };
    let current = |agent: &str| format!("cairn/{SERVER_VERSION} ({agent})");
    hook("working", &current("codex; hook")).await;
    hook("old", "cairn/0.0.1 (claude; hook)").await;
    hook("legacy", "Python-urllib/3.12").await;
    hook("quiet", &current("opencode; hook")).await;
    hook("mcp", &current("mcp")).await;
    sql(
        &app,
        "UPDATE devices SET last_hook_at = now() - interval '8 days' WHERE name = 'quiet'",
    )
    .await;

    let list = app.get("devices", &admin).await;
    let by = |name: &str| {
        list.as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == name)
            .unwrap()
            .clone()
    };
    let never = by("never");
    assert_eq!(never["health"], "never_connected");
    assert!(
        never["fix_command"]
            .as_str()
            .unwrap()
            .starts_with("cairn setup ")
    );
    let working = by("working");
    assert_eq!(
        (working["health"].as_str(), working["agent"].as_str()),
        (Some("working"), Some("codex"))
    );
    assert_eq!(working["client_version"], SERVER_VERSION);
    assert!(working["last_hook_at"].is_i64());
    assert!(working["fix_command"].is_null() && working["health_reason"].is_null());
    let old = by("old");
    assert_eq!(old["health"], "update_needed");
    assert_eq!(old["fix_command"], "cairn update");
    let reason = old["health_reason"].as_str().unwrap();
    assert!(
        reason.contains("0.0.1") && reason.contains(SERVER_VERSION),
        "{reason}"
    );
    let legacy = by("legacy");
    assert_eq!(legacy["health"], "update_needed");
    assert!(legacy["health_reason"].as_str().unwrap().contains("Python"));
    assert!(
        legacy["fix_command"]
            .as_str()
            .unwrap()
            .starts_with("cairn setup ")
    );
    assert_eq!(by("quiet")["health"], "stale");
    // An MCP call identifies the client but is not a hook check-in.
    let mcp = by("mcp");
    assert_eq!(mcp["client_version"], SERVER_VERSION);
    assert!(mcp["last_hook_at"].is_null());
    assert_eq!(mcp["health"], "working");
    // Revoked devices carry no health.
    let id = never["device_id"].as_str().unwrap();
    app.post("devices/revoke", json!({"deviceId": id}), &admin)
        .await;
    let revoked = by_name(&app.get("devices", &admin).await, "never");
    assert_eq!(revoked["status"], "revoked");
    assert!(revoked["health"].is_null());
    // The hook check-in is throttled like last_seen_at: a call 30 s after the last does not move it.
    sql(
        &app,
        "UPDATE devices SET last_hook_at = now() - interval '30 seconds' WHERE name = 'working'",
    )
    .await;
    hook("working", &current("codex; hook")).await;
    let moved: bool = sqlx::query_scalar(
        "SELECT last_hook_at > now() - interval '10 seconds' FROM devices WHERE name = 'working'",
    )
    .fetch_one(app.db.store.pool())
    .await
    .unwrap();
    assert!(!moved);
    sql(
        &app,
        "UPDATE devices SET last_hook_at = now() - interval '5 minutes' WHERE name = 'working'",
    )
    .await;
    hook("working", &current("codex; hook")).await;
    let moved: bool = sqlx::query_scalar(
        "SELECT last_hook_at > now() - interval '10 seconds' FROM devices WHERE name = 'working'",
    )
    .fetch_one(app.db.store.pool())
    .await
    .unwrap();
    assert!(moved);
    app.finish().await;
}

fn by_name(list: &Value, name: &str) -> Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == name)
        .unwrap()
        .clone()
}

#[tokio::test]
async fn health_reports_public_url_and_version() {
    let Some(app) = App::with_public_url("https://cairn.example.com/").await else {
        return;
    };
    let health = app.ok("GET", "health", json!(null), None).await;
    assert_eq!(health["public_url"], "https://cairn.example.com");
    assert_eq!(health["version"], SERVER_VERSION);
    app.finish().await;

    // With no CAIRN_PUBLIC_URL the server's own address is used.
    let Some(app) = App::new().await else { return };
    let (_, body) = app
        .send_with(
            "GET",
            "health",
            json!(null),
            None,
            &[("host", "cairn.internal:3111")],
        )
        .await;
    assert_eq!(body["public_url"], "http://cairn.internal:3111");
    let (_, body) = app
        .send_with(
            "GET",
            "health",
            json!(null),
            None,
            &[
                ("host", "cairn.internal:3111"),
                ("x-forwarded-host", "cairn.example.org"),
                ("x-forwarded-proto", "https"),
            ],
        )
        .await;
    assert_eq!(body["public_url"], "https://cairn.example.org");
    // The console's proxy forwards the browser's origin; Traefik's prefix strip says where agents go.
    let (_, body) = app
        .send_with(
            "GET",
            "health",
            json!(null),
            None,
            &[
                ("host", "cairn-server:3111"),
                ("x-forwarded-host", "cairn.example.org"),
                ("x-forwarded-proto", "https"),
                ("x-forwarded-prefix", "/agent/"),
            ],
        )
        .await;
    assert_eq!(body["public_url"], "https://cairn.example.org/agent");
    let (_, body) = app
        .send_with(
            "GET",
            "health",
            json!(null),
            None,
            &[
                ("host", "cairn.internal:3111"),
                ("x-forwarded-prefix", "/a b?x=1"),
            ],
        )
        .await;
    assert_eq!(
        body["public_url"], "http://cairn.internal:3111",
        "a bad prefix is ignored"
    );
    app.finish().await;
}

#[tokio::test]
async fn export_owned_memories_and_import_with_dry_run() {
    let Some(app) = App::new().await else { return };
    let alice = app.admin("alice").await;
    let bob = app.member(&alice, "bob").await;
    let carol = app.member(&alice, "carol").await;
    let (alice_d, bob_d) = (app.device(&alice, "a").await, app.device(&bob, "b").await);
    let project = app.project(&alice_d, "github.com/acme/widgets").await;
    app.post(
        "projects/init",
        json!({"project_id": project, "repository": "x"}),
        &bob_d,
    )
    .await;
    let personal = {
        let mut body = rule("Alice personal rule");
        body["scope"] = json!("personal");
        body["detail"] = json!("Expanded context from Alice's personal archive.");
        app.save(&alice_d, body).await
    };
    let global = {
        let mut body = rule("Alice global rule");
        body["scope"] = json!("global");
        app.save(&alice_d, body).await
    };
    app.save(
        &alice_d,
        in_project(rule("Alice project rule"), &project, "main", &[]),
    )
    .await;
    app.save(
        &bob_d,
        in_project(rule("Bob project rule"), &project, "main", &[]),
    )
    .await;
    let forgotten = app
        .save(&alice_d, on(rule("Alice forgotten rule"), "main"))
        .await;
    app.post("forget", json!({"id": forgotten}), &alice_d).await;

    let (status, headers, bytes) = app
        .send_raw("GET", "memories/export", json!(null), Some(&alice), &[])
        .await;
    assert_eq!(status, 200);
    let disposition = headers["content-disposition"].to_str().unwrap();
    assert!(
        disposition.starts_with("attachment; filename=\"cairn-export-")
            && disposition.ends_with(".json\""),
        "{disposition}"
    );
    let export: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(export["version"], 1);
    assert!(export["exported_at"].is_i64());
    let mut titles: Vec<&str> = export["memories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["title"].as_str().unwrap())
        .collect();
    titles.sort_unstable();
    assert_eq!(
        titles,
        [
            "Alice global rule",
            "Alice personal rule",
            "Alice project rule"
        ],
        "owned only, nothing forgotten"
    );
    let first = &export["memories"][0];
    let exported = export["memories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == personal)
        .unwrap();
    assert_eq!(
        exported["detail"],
        "Expanded context from Alice's personal archive."
    );
    for key in [
        "id",
        "kind",
        "scope",
        "status",
        "title",
        "summary",
        "reason",
        "fields",
        "evidence",
        "links",
        "project_id",
    ] {
        assert!(first.get(key).is_some(), "{key} missing: {first}");
    }
    // Export is a console action: not for a device token, not without a session.
    for token in [Some(alice_d.as_str()), None] {
        let (status, body) = app.send("GET", "memories/export", json!(null), token).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (401, Some("session_required"))
        );
    }

    // Carol imports Alice's file: the project memory is refused (carol is not a member).
    let mut file = export.clone();
    file["memories"].as_array_mut().unwrap().push(json!({"kind": "handoff", "scope": "personal", "title": "A handoff", "summary": "system only"}));
    file["memories"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind": "rule", "scope": "personal", "title": "No summary here"}));
    let (status, dry) = app
        .send(
            "POST",
            "memories/import?dry_run=1",
            file.clone(),
            Some(&carol),
        )
        .await;
    assert_eq!(status, 200, "{dry}");
    assert_eq!(dry["new"], 2, "{dry}");
    assert_eq!(dry["duplicate"], 0);
    let invalid = dry["invalid"].as_array().unwrap();
    assert_eq!(invalid.len(), 3, "{dry}");
    assert!(
        invalid
            .iter()
            .all(|i| i["index"].is_i64() && !i["error"].as_str().unwrap().is_empty())
    );
    let count = |app: &App| {
        let pool = app.db.store.pool().clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM memories WHERE owner_id = (SELECT person_id FROM accounts WHERE email = 'carol@example.test')")
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(count(&app).await, 0, "a dry run writes nothing");
    let (status, done) = app
        .send("POST", "memories/import", file.clone(), Some(&carol))
        .await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(
        (done["new"].as_i64(), done["duplicate"].as_i64()),
        (Some(2), Some(0))
    );
    assert_eq!(count(&app).await, 2);
    let again = app.post("memories/import", file.clone(), &carol).await;
    assert_eq!(
        (again["new"].as_i64(), again["duplicate"].as_i64()),
        (Some(0), Some(2)),
        "dedupe by identity"
    );
    let carols = app.get("memories", &carol).await;
    let mine = carols
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["owner_is_me"] == true)
        .count();
    assert_eq!(mine, 2, "imported rows are carol's own new memories");
    let imported = carols
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["title"] == "Alice personal rule")
        .unwrap();
    assert!(imported.get("detail").is_none());
    let detail = app
        .get(
            &format!("memories/{}", imported["id"].as_str().unwrap()),
            &carol,
        )
        .await;
    assert_eq!(detail["detail"], exported["detail"]);
    assert!(
        ids(&carols).contains(&global),
        "alice's global rule is visible to everyone"
    );
    let alices = carols
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == global.as_str())
        .unwrap();
    assert_eq!(
        alices["owner"], "",
        "another person's id is not shown: {alices}"
    );
    assert!(
        !ids(&carols).contains(&personal),
        "alice's personal rule stays hers"
    );
    // Bad bodies and device tokens.
    let (status, body) = app
        .send(
            "POST",
            "memories/import",
            json!({"version": 2, "memories": []}),
            Some(&carol),
        )
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (400, Some("validation")),
        "{body}"
    );
    let (status, _) = app
        .send(
            "POST",
            "memories/import",
            json!({"version": 1}),
            Some(&carol),
        )
        .await;
    assert_eq!(status, 400);
    let (status, body) = app
        .send("POST", "memories/import", file, Some(&alice_d))
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (401, Some("session_required"))
    );
    app.finish().await;
}

#[tokio::test]
async fn a_project_member_imports_a_project_memory_and_export_skips_handoffs() {
    let Some(app) = App::new().await else { return };
    let alice = app.admin("alice").await;
    let bob = app.member(&alice, "bob").await;
    let (alice_d, bob_d) = (app.device(&alice, "a").await, app.device(&bob, "b").await);
    let project = app.project(&alice_d, "github.com/acme/widgets").await;
    app.post(
        "projects/init",
        json!({"project_id": project, "repository": "x"}),
        &bob_d,
    )
    .await;
    app.save(
        &alice_d,
        in_project(rule("Alice project rule"), &project, "main", &[]),
    )
    .await;
    let handoff = app.save(&alice_d, on(rule("Alice handoff"), "main")).await;
    sql(
        &app,
        &format!("UPDATE memories SET kind = 'handoff' WHERE memory_id = '{handoff}'"),
    )
    .await;
    let (_, _, bytes) = app
        .send_raw("GET", "memories/export", json!(null), Some(&alice), &[])
        .await;
    let export: Value = serde_json::from_slice(&bytes).unwrap();
    let titles: Vec<&str> = export["memories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Alice project rule"], "no handoff in an export");

    let done = app.post("memories/import", export.clone(), &bob).await;
    assert_eq!(
        (
            done["new"].as_i64(),
            done["invalid"].as_array().map(Vec::len)
        ),
        (Some(1), Some(0)),
        "{done}"
    );
    let bobs = app.get("memories", &bob).await;
    let mine: Vec<&Value> = bobs
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["owner_is_me"] == true)
        .collect();
    assert_eq!(mine.len(), 1);
    assert_eq!(
        (mine[0]["title"].as_str(), mine[0]["project_id"].as_str()),
        (Some("Alice project rule"), Some(project.as_str()))
    );
    let again = app.post("memories/import", export, &bob).await;
    assert_eq!(again["duplicate"], 1, "importing twice adds nothing");
    // Only the import that wrote something is recorded.
    let audit = app.get("auth/audit?type=memories_imported", &bob).await;
    let audit = audit.as_array().unwrap();
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0]["metadata"]["new"], 1);
    app.finish().await;
}
