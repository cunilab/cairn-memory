//! Scripted agent-behaviour checks (spec 0.1.6 step 3, §8.2): each scenario is a fixed list of
//! MCP `tools/call` requests with asserted replies, against a real server. No model runs.

mod common;

use common::*;
use serde_json::{Value, json};

const CARD_KEYS: [&str; 8] = [
    "id", "kind", "project", "score", "summary", "title", "trust", "why",
];

fn decision() -> Value {
    json!({"kind": "decision", "title": "Use Postgres SKIP LOCKED for the job queue",
        "summary": "Background jobs run from a Postgres table claimed with SKIP LOCKED.",
        "reason": "Workers never block each other and there is one datastore.",
        "fields": {"context": "job queue", "alternatives": ["Redis"], "consequences": "one datastore"}})
}

/// Scenario 1: search when needed, then get; the fetch count is search plus get.
#[test]
fn search_when_needed_then_get() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "behave-search");
    let (dir, _, _) = shared_project(&team, "behave-search");
    let mut mcp = team.owner.mcp_in(&dir);
    let saved = mcp.ok("memory_save", decision());
    let id = saved["id"].as_str().unwrap().to_owned();
    mcp.ok(
        "memory_save",
        json!({"kind": "gotcha", "title": "Migrations hold a lock on startup",
            "summary": "Two servers starting together wait on the migration lock.",
            "reason": "Seen in the compose stack.",
            "fields": {"symptom": "second server hangs", "cause": "advisory lock", "fix": "start one first"}}),
    );
    mcp.ok(
        "memory_save",
        json!({"kind": "procedure", "title": "Run the web checks",
            "summary": "Typecheck, lint and build the web app before pushing.",
            "reason": "CI runs the same three.",
            "fields": {"steps": ["npm run typecheck", "npm run lint", "npm run build"], "when": "before a push"}}),
    );

    let result = mcp.tool("memory_search", json!({"query": "why skip locked"}));
    assert_eq!(
        result.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["content"]
    );
    let reply: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(
        reply.get("omitted").is_none() && reply.get("ids").is_none(),
        "{reply}"
    );
    let cards = reply["results"].as_array().unwrap();
    assert_eq!(cards[0]["id"], id.as_str());
    for card in cards {
        let mut keys: Vec<_> = card
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, CARD_KEYS, "{card}");
    }

    let got = mcp.ok("memory_get", json!({"ids": [id]}));
    let record = &got["memories"][0];
    assert_eq!(record["id"], id.as_str());
    assert_eq!(
        record["reason"],
        "Workers never block each other and there is one datastore."
    );
    assert_eq!(record["fields"]["alternatives"], json!(["Redis"]));
    assert_eq!(server.fetched(&team.owner_id, &id), 2);
}

/// Scenario 2: a save needs a reason; the same body twice is deduplicated.
#[test]
fn save_with_a_reason() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "behave-save");
    let (dir, _, _) = shared_project(&team, "behave-save");
    let mut mcp = team.owner.mcp_in(&dir);
    let mut body = decision();
    body.as_object_mut().unwrap().remove("reason");

    let refused = mcp.tool("memory_save", body);
    assert_eq!(refused["isError"], true, "{refused}");
    let text = refused["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("reason") && !text.contains('\n'), "{text}");

    let mut body = decision();
    body["summary"] = json!("x".repeat(263));
    let refused = mcp.tool("memory_save", body);
    assert_eq!(refused["isError"], true, "{refused}");
    assert_eq!(
        refused["content"][0]["text"],
        "HTTP 400: summary is 263 characters, limit 240: cut 23, or move extra context into reason or detail"
    );

    let first = mcp.ok("memory_save", decision());
    assert_eq!(first["status"], "active");
    assert_eq!(first["deduplicated"], false);
    let again = mcp.ok("memory_save", decision());
    assert_eq!(again["id"], first["id"]);
    assert_eq!(again["deduplicated"], true);
}

/// Scenario 3: a correction is a save with `supersedes`; both stay searchable and the new one ranks first.
#[test]
fn correct_via_supersedes() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "behave-supersede");
    let (dir, _, _) = shared_project(&team, "behave-supersede");
    let mut mcp = team.owner.mcp_in(&dir);
    let old = mcp.ok("memory_save", decision())["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut better = decision();
    better["title"] = json!("Use Postgres SKIP LOCKED with a lease column");
    better["summary"] = json!("Jobs are claimed with SKIP LOCKED and a lease column.");
    better["supersedes"] = json!(old);
    let new = mcp.ok("memory_save", better)["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let got = mcp.ok("memory_get", json!({"ids": [old]}));
    assert_eq!(got["memories"][0]["status"], "superseded");
    let links = got["memories"][0]["links"].as_array().unwrap();
    assert!(
        links.iter().any(|link| link["relation"] == "supersedes"
            && link["source"] == new.as_str()
            && link["target"] == old.as_str()),
        "{links:?}"
    );
    let found = mcp.ok("memory_search", json!({"query": "skip locked"}));
    let ids: Vec<_> = found["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|card| card["id"].as_str().unwrap())
        .collect();
    let new_at = ids.iter().position(|id| *id == new.as_str());
    let old_at = ids.iter().position(|id| *id == old.as_str());
    assert!(
        new_at.is_some() && old_at.is_some() && new_at.unwrap() < old_at.unwrap(),
        "{ids:?}"
    );
    let old_card = found["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|card| card["id"] == old)
        .unwrap();
    assert_eq!(old_card["label"], format!("replaced by {new}"));
}

#[test]
fn memory_save_coaches_retry_and_explicit_correction() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "behave-coaching");
    let dir = checkout("behave-coaching");
    assert!(
        team.owner
            .run_in(&dir, &["init", "E2E Save coaching"])
            .status
            .success()
    );
    let mut mcp = team.owner.mcp_in(&dir);

    let mut weak = decision();
    weak["reason"] = json!("Checked.");
    let saved = mcp.ok("memory_save", weak.clone());
    let id = saved["id"].as_str().unwrap();
    assert_eq!(saved["status"], "active");
    assert_eq!(saved["hints"].as_array().unwrap().len(), 1);
    assert_eq!(
        saved["improve"],
        format!("Improve it: save again with supersedes={id}.")
    );
    let repeat = mcp.ok("memory_save", weak);
    assert_eq!(repeat["id"], id);
    assert_eq!(repeat["deduplicated"], true);
    assert_eq!(repeat["hints"], saved["hints"]);
    for _ in 0..2 {
        let read = mcp.ok("memory_get", json!({"ids": [id]}));
        assert!(
            read["memories"][0]["trust"]
                .as_str()
                .unwrap()
                .starts_with("new")
        );
    }

    // Reason is outside identity, so this explicit correction updates the same row.
    let mut better = decision();
    better["supersedes"] = json!(id);
    let corrected = mcp.ok("memory_save", better.clone());
    assert_eq!(
        corrected,
        json!({"id": id, "status": "active", "deduplicated": true})
    );
    let read = mcp.ok("memory_get", json!({"ids": [id]}));
    assert!(
        read["memories"][0]["trust"]
            .as_str()
            .unwrap()
            .starts_with("normal")
    );
    assert_eq!(read["memories"][0]["reason"], better["reason"]);

    better.as_object_mut().unwrap().remove("supersedes");
    better["summary"] =
        json!("The background jobs run from a Postgres table claimed with SKIP LOCKED.");
    let close = mcp.ok("memory_save", better);
    let close_id = close["id"].as_str().unwrap();
    assert_ne!(close_id, id);
    let [hint] = close["hints"].as_array().unwrap().as_slice() else {
        panic!("{close}");
    };
    let hint = hint.as_str().unwrap();
    assert!(hint.starts_with(&format!("Looks like {id} 'Use Postgres SKIP LOCKED")));
    assert!(hint.ends_with(&format!("Use supersedes={id}.")));
    let read = mcp.ok("memory_get", json!({"ids": [close_id]}));
    assert_eq!(read["memories"][0]["trust"], "new · awaiting confirmation");
    mcp.ok(
        "memory_feedback",
        json!({"id": close_id, "vote": "helpful"}),
    );
    let read = mcp.ok("memory_get", json!({"ids": [close_id, id]}));
    assert!(
        read["memories"][0]["trust"]
            .as_str()
            .unwrap()
            .starts_with("normal")
    );
    assert_eq!(read["memories"][1]["status"], "active");
    for path in [
        dir.parent().unwrap(),
        team.owner.home.as_path(),
        team.member.home.as_path(),
    ] {
        std::fs::remove_dir_all(path).unwrap();
    }
}

/// Scenario 4: two wrong votes from a member make a memory stale; its owner can forget it.
#[test]
fn feedback_and_forget() {
    let Some(server) = Server::start() else {
        return;
    };
    let team = team(&server.url, "behave-feedback");
    let (dir_a, dir_b, _) = shared_project(&team, "behave-feedback");
    let mut owner = team.owner.mcp_in(&dir_a);
    let mut member = team.member.mcp_in(&dir_b);
    let id = owner.ok("memory_save", decision())["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let first = member.ok("memory_feedback", json!({"id": id, "vote": "wrong"}));
    assert_eq!(first["status"], "active");
    // The same voter again changes nothing; the owner's own `wrong` makes it stale.
    let again = member.ok("memory_feedback", json!({"id": id, "vote": "wrong"}));
    assert_eq!(again["status"], "active");
    let owner_vote = owner.ok("memory_feedback", json!({"id": id, "vote": "wrong"}));
    assert_eq!(owner_vote["status"], "stale");

    let forgotten = owner.ok("memory_forget", json!({"id": id}));
    assert_eq!(forgotten["status"], "forgotten");
    let got = owner.ok("memory_get", json!({"ids": [id]}));
    assert_eq!(got["missing"], json!([id]));
}
