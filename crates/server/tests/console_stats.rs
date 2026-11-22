//! `GET /stats`: the dashboard's numbers, current values only, for the caller's visible memories.

mod common;
use common::App;
use serde_json::{Value, json};

fn rule(title: &str) -> Value {
    json!({"kind": "rule", "title": title, "summary": format!("{title} is the rule here."), "reason": "the test needs a reason"})
}

fn in_project(mut body: Value, project: &str) -> Value {
    body["context"] =
        json!({"project_id": project, "repository": "github.com/acme/widgets", "branch": "main"});
    body
}

async fn sql(app: &App, query: &str) {
    sqlx::query(query)
        .execute(app.db.store.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn stats_counts_only_visible() {
    let Some(app) = App::new().await else { return };
    let alice = app.admin("alice").await;
    let bob = app.member(&alice, "bob").await;
    let carol = app.member(&alice, "carol").await;
    let (alice_d, bob_d, carol_d) = (
        app.device(&alice, "a").await,
        app.device(&bob, "b").await,
        app.device(&carol, "c").await,
    );
    let project = app.project(&alice_d, "github.com/acme/widgets").await;
    app.post(
        "projects/init",
        json!({"project_id": project, "repository": "x"}),
        &bob_d,
    )
    .await;
    app.save(&alice_d, in_project(rule("Project rule one"), &project))
        .await;
    app.save(&alice_d, in_project(rule("Project rule two"), &project))
        .await;
    app.save(&alice_d, with_scope(rule("Alice private rule"), "personal"))
        .await;
    app.save(&alice_d, with_scope(rule("Shared global rule"), "global"))
        .await;
    app.save(&bob_d, with_scope(rule("Bob private rule"), "personal"))
        .await;
    app.save(&carol_d, with_scope(rule("Carol private rule"), "personal"))
        .await;

    let bob_stats = app.get("stats", &bob).await;
    // Bob sees the two project rules, the global rule and his own: four.
    assert_eq!(bob_stats["memories"]["total"], 4, "{bob_stats}");
    assert_eq!(
        bob_stats["memories"]["by_scope"],
        json!({"global": 1, "personal": 1, "project": 2})
    );
    assert_eq!(bob_stats["memories"]["by_kind"], json!({"rule": 4}));
    assert_eq!(bob_stats["memories"]["by_status"], json!({"active": 4}));
    assert_eq!(bob_stats["memories"]["by_origin"], json!({"agent": 4}));
    assert_eq!(bob_stats["projects"], 1);
    let carol_stats = app.get("stats", &carol).await;
    assert_eq!(carol_stats["memories"]["total"], 2, "{carol_stats}");
    assert_eq!(carol_stats["projects"], 0);
    // Every contract field is present, with current values only.
    for key in [
        "memories",
        "activity",
        "usage",
        "usage_days",
        "bench",
        "stale",
        "links",
        "memory_health",
        "jobs",
        "projects",
        "devices_attention",
        "first_run",
        "capture",
    ] {
        assert!(!bob_stats[key].is_null(), "{key} missing: {bob_stats}");
    }
    for old in ["baseline", "previous", "compare", "delta"] {
        assert!(
            !bob_stats.to_string().contains(old),
            "{old} must not appear"
        );
    }
    app.finish().await;
}

/// The UTC date `offset` days ago, as the database sees it.
async fn utc_day(app: &App, offset: i32) -> String {
    sqlx::query_scalar::<_, String>(&format!(
        "SELECT to_char((now() AT TIME ZONE 'UTC')::date - {offset}, 'YYYY-MM-DD')"
    ))
    .fetch_one(app.db.store.pool())
    .await
    .unwrap()
}

fn with_scope(mut body: Value, scope: &str) -> Value {
    body["scope"] = json!(scope);
    body
}

#[tokio::test]
async fn stats_activity_zero_fills_30_days_in_tz() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "d").await;
    let id = app.save(&device, rule("Saved two days ago")).await;
    // 23:30 UTC two days ago: still that day at UTC, already the next day at UTC+1.
    sql(
        &app,
        &format!("UPDATE memories SET created_at = date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' - interval '2 days' + interval '23 hours 30 minutes' WHERE memory_id = '{id}'"),
    )
    .await;
    let (today, d1, d2) = (
        utc_day(&app, 0).await,
        utc_day(&app, 1).await,
        utc_day(&app, 2).await,
    );
    let at_utc = app.get("stats?tz=0", &admin).await;
    let days = at_utc["activity"].as_array().unwrap();
    assert_eq!(days.len(), 30);
    assert_eq!(days[29]["day"], today);
    assert_eq!(days[0]["saved"], 0);
    let saved = |days: &[Value], d: &str| {
        days.iter()
            .find(|x| x["day"] == d)
            .map(|x| x["saved"].as_i64().unwrap())
    };
    assert_eq!(saved(days, &d2), Some(1));
    assert_eq!(saved(days, &d1), Some(0));
    let east = app.get("stats?tz=60", &admin).await;
    let days = east["activity"].as_array().unwrap();
    assert_eq!(saved(days, &d1), Some(1), "{east}");
    assert_eq!(saved(days, &d2), Some(0));
    // The shorter ranges and the bounds.
    assert_eq!(
        app.get("stats?days=7", &admin).await["activity"]
            .as_array()
            .unwrap()
            .len(),
        7
    );
    assert_eq!(
        app.get("stats?days=90", &admin).await["usage_days"]["days"]
            .as_array()
            .unwrap()
            .len(),
        90
    );
    for bad in ["stats?days=8", "stats?tz=841", "stats?tz=-841"] {
        let (status, body) = app.send("GET", bad, json!(null), Some(&admin)).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (400, Some("validation")),
            "{bad}"
        );
    }
    app.finish().await;
}

#[tokio::test]
async fn stats_links_ratio_matches_sql() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "d").await;
    let a = app.save(&device, rule("Linked rule alpha")).await;
    let b = app.save(&device, rule("Linked rule bravo")).await;
    app.save(&device, rule("Lonely rule charlie")).await;
    sql(&app, &format!("INSERT INTO links (source_id, target_id, relation, origin) VALUES ('{a}', '{b}', 'relates', 'auto')")).await;
    let stats = app.get("stats", &admin).await;
    assert_eq!(stats["links"]["total"], 1);
    assert_eq!(stats["links"]["auto"], 1);
    let ratio = stats["links"]["durable_with_link_ratio"].as_f64().unwrap();
    assert!((ratio - 2.0 / 3.0).abs() < 0.01, "{ratio}");
    assert_eq!(
        stats["memory_health"]["linked_ratio"],
        stats["links"]["durable_with_link_ratio"]
    );
    app.finish().await;
}

#[tokio::test]
async fn stats_usage_stale_expiry_and_jobs() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let other = app.member(&admin, "other").await;
    let device = app.device(&admin, "d").await;
    let a = app.save(&device, rule("Most used rule")).await;
    let b = app.save(&device, rule("Used once rule")).await;
    let c = app.save(&device, rule("Stale missing rule")).await;
    let d = app.save(&device, rule("Stale feedback rule")).await;
    let e = app.save(&device, rule("Expiring soon rule")).await;
    let f = app.save(&device, rule("Expired already rule")).await;
    app.post("get", json!({"ids": [a]}), &device).await;
    app.post("get", json!({"ids": [a, b]}), &device).await;
    app.post("feedback", json!({"id": a, "vote": "helpful"}), &device)
        .await;
    // Reading one memory through the console never counts as a fetch.
    app.get(&format!("memories/{c}"), &admin).await;
    sql(
        &app,
        &format!(
            "UPDATE memories SET status = 'stale', stale_cause = 'missing' WHERE memory_id = '{c}'"
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
    sql(
        &app,
        &format!(
            "UPDATE memories SET expires_at = now() + interval '10 days' WHERE memory_id = '{e}'"
        ),
    )
    .await;
    sql(
        &app,
        &format!(
            "UPDATE memories SET expires_at = now() - interval '2 days' WHERE memory_id = '{f}'"
        ),
    )
    .await;

    let stats = app.get("stats", &admin).await;
    assert_eq!(
        stats["memories"]["total"], 6,
        "an old expires_at stays in the count: {stats}"
    );
    assert_eq!(stats["memories"]["expired_30d"], 1);
    assert_eq!(stats["usage"]["fetched_30d"], 2);
    assert_eq!(stats["usage"]["helpful_30d"], 1);
    // Durable and active: a, b, e (c and d are stale). Fetched: a, b.
    assert_eq!(stats["usage"]["never_fetched"], 2);
    let top = stats["usage"]["top"].as_array().unwrap();
    assert_eq!(top[0]["id"], a);
    assert_eq!(
        (top[0]["fetched"].as_i64(), top[0]["helpful"].as_i64()),
        (Some(2), Some(1))
    );
    assert_eq!(top[1]["id"], b);
    assert_eq!(top[0]["kind"], "rule");
    assert_eq!(top[0]["title"], "Most used rule");
    assert_eq!(stats["stale"]["total"], 2);
    assert_eq!(
        stats["stale"]["by_cause"],
        json!({"missing": 1, "feedback": 1})
    );
    assert_eq!(stats["stale"]["expiring_30d"], 1);
    assert_eq!(stats["memory_health"]["expiring"], 1);
    assert_eq!(stats["memory_health"]["stale_missing"], 1);
    assert_eq!(stats["memory_health"]["stale_feedback"], 1);
    // Six saves queued embed and link jobs; none is quarantined.
    assert!(stats["jobs"]["pending"].as_i64().unwrap() >= 6);
    assert_eq!(stats["jobs"]["quarantined"], 0);
    // Another person's counts are their own.
    let theirs = app.get("stats", &other).await;
    assert_eq!(theirs["memories"]["total"], 0);
    assert_eq!(theirs["jobs"]["pending"], 0);
    app.finish().await;
}

#[tokio::test]
async fn stats_usage_days_count_lookups_hits_and_tokens() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "d").await;
    let a = app.save(&device, rule("Cache invalidation rule")).await;
    let hit = app
        .post("search", json!({"query": "cache invalidation"}), &device)
        .await;
    assert_eq!(hit["results"][0]["id"], a);
    let miss = app
        .post("search", json!({"query": "zzqqxx nothing here"}), &device)
        .await;
    assert!(miss["results"].as_array().unwrap().is_empty());
    app.post("get", json!({"ids": [a]}), &device).await;
    app.post("session/brief", json!({"session_id": "s1"}), &device)
        .await;
    let stats = app.get("stats", &admin).await;
    let usage = &stats["usage_days"];
    assert_eq!(usage["lookups"], 4, "{usage}");
    let ratio = usage["hit_ratio"].as_f64().unwrap();
    assert!(
        (0.5..=0.75).contains(&ratio),
        "search hit, search miss, get hit, brief ?: {ratio}"
    );
    let days = usage["days"].as_array().unwrap();
    assert_eq!(days.len(), 30);
    let tokens: i64 = days.iter().map(|d| d["tokens"].as_i64().unwrap()).sum();
    assert_eq!(usage["total_tokens"], tokens);
    assert!(tokens > 0);
    let last = days.last().unwrap();
    assert_eq!(last["lookups"], 4);
    assert!(last["hits"].as_i64().unwrap() >= 2);
    // The benchmark card is the committed v2 run, current values only.
    assert_eq!(stats["bench"]["hit_at_5"], 0.6512);
    assert_eq!(stats["bench"]["search_tokens"], 133);
    assert_eq!(stats["bench"]["get_tokens"], 266);
    assert!(stats["bench"]["session_tokens"].as_i64().unwrap() > 1000);
    app.finish().await;
}

#[tokio::test]
async fn stats_first_run_capture_and_devices_attention() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let fresh = app.get("stats", &admin).await;
    assert_eq!(fresh["first_run"], true);
    assert!(fresh["capture"]["last_at"].is_null());
    assert_eq!(fresh["capture"]["observations_7d"], 0);
    assert_eq!(fresh["devices_attention"], json!([]));

    let device = app.device(&admin, "laptop-claude").await;
    // A device token alone is not a check-in; a recognised client user agent is.
    let old = "cairn/0.0.1 (claude; hook)";
    app.send_with(
        "POST",
        "session/brief",
        json!({"session_id": "s1"}),
        Some(&device),
        &[("user-agent", old)],
    )
    .await;
    app.post(
        "observe",
        json!({"event_id": "e1", "session_id": "s1", "kind": "prompt", "content": "TODO: first capture"}),
        &device,
    )
    .await;
    let stats = app.get("stats", &admin).await;
    assert_eq!(stats["first_run"], false, "{stats}");
    assert!(stats["capture"]["last_at"].is_i64());
    assert_eq!(stats["capture"]["observations_7d"], 1);
    let attention = stats["devices_attention"].as_array().unwrap();
    assert_eq!(attention.len(), 1, "{stats}");
    assert_eq!(attention[0]["name"], "laptop-claude");
    assert!(attention[0]["reason"].as_str().unwrap().contains("0.0.1"));
    assert!(
        attention[0]["device_id"]
            .as_str()
            .unwrap()
            .starts_with("dev_")
    );
    app.finish().await;
}

#[tokio::test]
async fn first_run_stays_off_after_every_device_is_revoked() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "laptop-claude").await;
    app.send_with(
        "POST",
        "session/brief",
        json!({"session_id": "s1"}),
        Some(&device),
        &[("user-agent", "cairn/0.1.6 (claude; hook)")],
    )
    .await;
    app.post(
        "observe",
        json!({"event_id": "e1", "session_id": "s1", "kind": "prompt", "content": "TODO: first capture"}),
        &device,
    )
    .await;
    assert_eq!(app.get("stats", &admin).await["first_run"], false);
    let listed = app.get("devices", &admin).await;
    let id = listed[0]["device_id"].as_str().unwrap();
    app.post("devices/revoke", json!({"deviceId": id}), &admin)
        .await;
    let stats = app.get("stats", &admin).await;
    assert_eq!(stats["devices_checked_in"], 0);
    assert_eq!(
        stats["first_run"], false,
        "months of history, no live device: {stats}"
    );
    app.finish().await;
}

#[tokio::test]
async fn attention_is_a_session_only_slice_of_stats() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let member = app.member(&admin, "member").await;
    let device = app.device(&admin, "laptop-claude").await;
    app.send_with(
        "POST",
        "session/brief",
        json!({"session_id": "s1"}),
        Some(&device),
        &[("user-agent", "cairn/0.0.1 (claude; hook)")],
    )
    .await;
    let attention = app.get("stats/attention", &admin).await;
    let stats = app.get("stats", &admin).await;
    assert_eq!(attention["devices_attention"], stats["devices_attention"]);
    assert_eq!(attention["devices_attention"].as_array().unwrap().len(), 1);
    assert_eq!(attention["jobs"], stats["jobs"]);
    assert_eq!(
        app.get("stats/attention", &member).await["devices_attention"],
        json!([])
    );
    for token in [Some(device.as_str()), None] {
        let (status, body) = app.send("GET", "stats/attention", json!(null), token).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (401, Some("session_required"))
        );
    }
    app.finish().await;
}
