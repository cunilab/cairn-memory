//! The semantic channel and the consolidate job, with the real pinned MiniLM model.
//! Needs `CAIRN_TEST_EMBEDDING_MODEL_DIR` (or `CAIRN_EMBEDDING_MODEL_DIR`); skipped without it
//! unless `CAIRN_TEST_REQUIRE_DB=1`.

mod common;
use cairn_server::LocalEmbeddings;
use cairn_store::retrieve::Narrow;
use common::{App, ids};
use serde_json::json;
use std::path::PathBuf;

fn model() -> Option<LocalEmbeddings> {
    let dir = std::env::var("CAIRN_TEST_EMBEDDING_MODEL_DIR")
        .or_else(|_| std::env::var("CAIRN_EMBEDDING_MODEL_DIR"));
    let Ok(dir) = dir else {
        assert!(
            std::env::var("CAIRN_TEST_REQUIRE_DB").as_deref() != Ok("1"),
            "CAIRN_TEST_REQUIRE_DB=1 but no embedding model directory is set"
        );
        println!("SKIP (no CAIRN_TEST_EMBEDDING_MODEL_DIR)");
        return None;
    };
    Some(LocalEmbeddings::load(&PathBuf::from(dir)).expect("load the pinned model"))
}

#[tokio::test]
async fn meaning_finds_memories_and_never_crosses_people() {
    let Some(model) = model() else { return };
    let Some(app) = App::with_model(Some(model)).await else {
        return;
    };
    assert_eq!(
        app.ok("GET", "health", json!(null), None).await["semantic_search"],
        "enabled"
    );
    let alice = app.admin("alice").await;
    let bob = app.member(&alice, "bob").await;
    let mine = app
        .post("remember", json!({"kind": "gotcha", "title": "Invoices render a blank page", "summary": "The typst template fails when a customer name has an emoji.", "reason": "font fallback", "scope": "personal", "fields": {"fix": "Strip emoji from the customer name."}}), &alice)
        .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    app.post("remember", json!({"kind": "fact", "title": "Release cadence", "summary": "We ship every second Tuesday.", "reason": "agreed and the test says why", "scope": "personal"}), &alice).await;
    app.settle().await;

    // The vector channel finds the memory when no word of the query appears in it...
    let paraphrase = "pdf bill shows empty sheet with unusual symbols";
    let vector = app
        .state
        .embeddings
        .as_ref()
        .unwrap()
        .query(paraphrase)
        .await
        .unwrap();
    let person = |me: &serde_json::Value| me["person_id"].as_str().unwrap().to_owned();
    let alice_id = person(&app.get("auth/me", &alice).await);
    let bob_id = person(&app.get("auth/me", &bob).await);
    let store = &app.db.store;
    let alice_projects = store.member_projects(&alice_id).await.unwrap();
    let bob_projects = store.member_projects(&bob_id).await.unwrap();
    let candidates = store
        .vector_candidates(&alice_id, &alice_projects, &vector, 50, &Narrow::NONE)
        .await
        .unwrap();
    assert_eq!(candidates.first().map(|(id, _)| id), Some(&mine));
    // ...and the visibility rule is applied to what it returns: bob gets nothing of alice's.
    assert!(
        store
            .vector_candidates(&bob_id, &bob_projects, &vector, 50, &Narrow::NONE)
            .await
            .unwrap()
            .is_empty()
    );
    // The gate keeps a paraphrase on cosine alone (`semantic_paraphrase_found`), for alice only.
    let found = app
        .post("search", json!({"query": paraphrase}), &alice)
        .await;
    assert_eq!(ids(&found).first(), Some(&mine), "{found}");
    assert!(ids(&app.post("search", json!({"query": paraphrase}), &bob).await).is_empty());
    // A query that shares words finds it for alice and reaches nothing for bob.
    let query = json!({"query": "invoices render blank page with emoji"});
    assert_eq!(
        ids(&app.post("search", query.clone(), &alice).await).first(),
        Some(&mine)
    );
    assert!(ids(&app.post("search", query, &bob).await).is_empty());
    let by_kind = app
        .post(
            "search",
            json!({"query": "invoice blank page", "kind": "gotcha"}),
            &bob,
        )
        .await;
    assert!(ids(&by_kind).is_empty(), "{by_kind}");
    app.finish().await;
}

#[tokio::test]
async fn consolidate_supersedes_near_duplicates_and_links_related() {
    let Some(model) = model() else { return };
    let Some(app) = App::with_model(Some(model)).await else {
        return;
    };
    let owner = app.admin("owner").await;
    let save = |title: &str, summary: &str| {
        app.post("remember", json!({"kind": "rule", "title": title, "summary": summary, "reason": "money and the test says why", "scope": "personal"}), &owner)
    };
    let old = save(
        "Store money as integer cents",
        "Never store money as floats; use integer cents everywhere.",
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    app.settle().await;
    let new = save(
        "Store money as integer cents.",
        "Never store money as floats; use integer cents everywhere!",
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let related = save(
        "Keep money as integer cents in the database",
        "Amounts are persisted as whole cents, never as floating point numbers.",
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    save(
        "Prices are stored in minor currency units",
        "Prices are kept as an integer number of minor currency units.",
    )
    .await;
    save(
        "Currency amounts use integer storage",
        "Do not use float columns for currency amounts.",
    )
    .await;
    app.settle().await;
    let old_memory = app.get(&format!("memories/{old}"), &owner).await;
    assert_eq!(old_memory["status"], "superseded", "{old_memory}");
    let links = old_memory["links"].as_array().unwrap();
    assert!(
        links.iter().any(|l| l["relation"] == "supersedes"
            && l["source"] == new.as_str()
            && l["auto"] == true),
        "{links:?}"
    );
    assert_eq!(
        app.get(&format!("memories/{new}"), &owner).await["status"],
        "active"
    );
    let near = app.get(&format!("memories/{related}"), &owner).await;
    assert!(
        near["links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["relation"] == "relates" && l["auto"] == true),
        "{near}"
    );
    // Another person's near-identical memory is left alone.
    let other = app.member(&owner, "other").await;
    app.post("remember", json!({"kind": "rule", "title": "Store money as integer cents", "summary": "Never store money as floats; use integer cents everywhere.", "reason": "money and the test says why", "scope": "personal"}), &other).await;
    app.settle().await;
    assert_eq!(
        app.get(&format!("memories/{new}"), &owner).await["status"],
        "active"
    );
    app.finish().await;
}

/// The gate's margin shrinks as a store grows (review I5): the nearest unrelated neighbour of an
/// off-topic query gets closer. With 400 everyday notes, an off-topic query, including one that
/// shares an ordinary word with a note, still returns nothing, and a paraphrase still finds its
/// memory. Real MiniLM embeddings of 400 short notes take a few seconds. This is a guard, not a
/// calibration: no constant was chosen with it.
#[tokio::test]
async fn off_topic_queries_stay_empty_among_400_distractors() {
    let Some(model) = model() else { return };
    let Some(app) = App::with_model(Some(model)).await else {
        return;
    };
    let me = app.admin("me").await;
    let topics = [
        "sourdough starter",
        "telescope mirror",
        "garden irrigation",
        "train timetable",
        "dentist visit",
        "bike repair",
        "language class",
        "book club",
        "guitar practice",
        "insurance renewal",
        "office lunch order",
        "parking permit",
        "recipe for leek soup",
        "running route",
        "moving checklist",
        "conference notes",
    ];
    let details = [
        "needs attention before the weekend",
        "was moved to the second Tuesday of the month",
        "costs a little more than last year",
        "is booked through the front desk",
        "should be compared against two other quotes",
        "is easier in the morning when it is quiet",
        "was agreed with the whole family",
        "stays the same until the end of spring",
    ];
    for i in 0..400 {
        let title = format!("{} {}", topics[i % topics.len()], 100 + i);
        let summary = format!(
            "The {} {}.",
            topics[i % topics.len()],
            details[(i / 2) % details.len()]
        );
        app.post("remember", json!({"kind": "fact", "title": title, "summary": summary, "reason": "note and the test says why", "scope": "personal"}), &me).await;
    }
    let target = app
        .post("remember", json!({"kind": "gotcha", "title": "Invoices render a blank page", "summary": "The typst template fails when a customer name has an emoji.", "reason": "font fallback", "scope": "personal", "fields": {"fix": "Strip emoji from the customer name."}}), &me)
        .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for _ in 0..200 {
        if app.db.store.pending_embeddings(1).await.unwrap().is_empty() {
            break;
        }
        app.settle().await;
    }
    for query in [
        "volcanic eruption evacuation plan",
        "quantum chromodynamics lattice simulation",
        "quarterly revenue forecast for the mining sector",
        "ancient Roman aqueduct engineering",
        // One ordinary word in common with the notes, nothing else.
        "tuning a diesel engine for a rally race",
        "plan the national budget for next year",
    ] {
        let found = app.post("search", json!({"query": query}), &me).await;
        assert!(ids(&found).is_empty(), "`{query}` returned cards: {found}");
    }
    let found = app
        .post(
            "search",
            json!({"query": "pdf bill shows empty sheet with unusual symbols"}),
            &me,
        )
        .await;
    assert_eq!(ids(&found).first(), Some(&target), "{found}");
    app.finish().await;
}

#[tokio::test]
async fn ranking_follows_the_match_not_the_kind_or_the_age_of_the_embedding() {
    let Some(model) = model() else { return };
    let Some(app) = App::with_model(Some(model)).await else {
        return;
    };
    let me = app.admin("alice").await;
    let save = |body: serde_json::Value| app.post("remember", body, &me);
    save(json!({"kind":"decision","title":"Cairn keeps MiniLM embeddings","summary":"Cairn 0.1.6 keeps the MiniLM embedding model; larger models did not improve end-to-end recall.","reason":"measured on bench v2","scope":"personal"})).await;
    save(json!({"kind":"gotcha","title":"cairn init 404 after wipe","summary":"After a server wipe cairn init returns 404 because the committed project.json names a project the server no longer has.","reason":"seen in a live test","scope":"personal","fields":{"fix":"Re-register the project with cairn init."}})).await;
    let old = save(json!({"kind":"procedure","title":"Release a Cairn version","summary":"Bump the version, merge to main, tag vX.Y.Z and let the release workflow publish assets.","reason":"release steps","scope":"personal","fields":{"steps":["Bump the workspace version","Merge to main","Push the tag"]}})).await;
    app.settle().await;
    let search = |query: &str| {
        let found = app.post("search", json!({"query": query}), &me);
        async move { found.await["results"].as_array().unwrap().clone() }
    };
    let title = |card: &serde_json::Value| card["title"].as_str().unwrap().to_owned();

    // The procedure matches both words and the intent; it is first, with a score that is a match
    // quality (not 1.0 because it is on top), and nothing that only shares "cairn" outranks it.
    for query in ["how do I release cairn", "release cairn"] {
        let cards = search(query).await;
        assert_eq!(
            title(&cards[0]),
            "Release a Cairn version",
            "{query}: {cards:?}"
        );
        let top = cards[0]["score"].as_f64().unwrap();
        assert!(top > 0.0 && top < 1.0, "{query}: absolute score, got {top}");
        for weaker in &cards[1..] {
            assert!(
                weaker["score"].as_f64().unwrap() < top,
                "{query}: {cards:?}"
            );
        }
    }

    // A superseding save is searchable at once, before its embedding exists, and still first.
    let new = save(json!({"kind":"procedure","title":"Release a Cairn version","summary":"Run scripts/release.sh with the new version, then tag vX.Y.Z and wait for the release workflow to publish assets.","reason":"the release steps changed","scope":"personal","supersedes": old["id"],"fields":{"steps":["Run scripts/release.sh","Push the tag"]}})).await;
    for settled in [false, true] {
        if settled {
            app.settle().await;
        }
        let cards = search("release cairn").await;
        assert_eq!(cards[0]["id"], new["id"], "settled {settled}: {cards:?}");
        assert!(cards.iter().all(|c| c["id"] != old["id"]));
    }
    app.finish().await;
}
