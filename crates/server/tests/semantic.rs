//! The semantic channel and the consolidate job, with the real pinned MiniLM model.
//! Needs `CAIRN_TEST_EMBEDDING_MODEL_DIR` (or `CAIRN_EMBEDDING_MODEL_DIR`); skipped without it
//! unless `CAIRN_TEST_REQUIRE_DB=1`.

mod common;
use cairn_server::LocalEmbeddings;
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
        .post("remember", json!({"kind": "gotcha", "title": "Invoices render a blank page", "summary": "The typst template fails when a customer name has an emoji.", "reason": "font fallback", "scope": "personal"}), &alice)
        .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    app.post("remember", json!({"kind": "fact", "title": "Release cadence", "summary": "We ship every second Tuesday.", "reason": "agreed", "scope": "personal"}), &alice).await;
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
        .vector_candidates(&alice_id, &alice_projects, &vector, 50)
        .await
        .unwrap();
    assert_eq!(candidates.first().map(|(id, _)| id), Some(&mine));
    // ...and the visibility rule is applied to what it returns: bob gets nothing of alice's.
    assert!(
        store
            .vector_candidates(&bob_id, &bob_projects, &vector, 50)
            .await
            .unwrap()
            .is_empty()
    );
    // At step 2 the relevance floor still drops a hit that shares no word with the query
    // (the baseline paraphrase hit@5 is 0.0 for the same reason); step 5 changes that.
    assert!(
        ids(&app
            .post("search", json!({"query": paraphrase}), &alice)
            .await)
        .is_empty()
    );
    // A query that clears the floor finds it for alice and reaches nothing for bob.
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
        app.post("remember", json!({"kind": "rule", "title": title, "summary": summary, "reason": "money", "scope": "personal"}), &owner)
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
    app.post("remember", json!({"kind": "rule", "title": "Store money as integer cents", "summary": "Never store money as floats; use integer cents everywhere.", "reason": "money", "scope": "personal"}), &other).await;
    app.settle().await;
    assert_eq!(
        app.get(&format!("memories/{new}"), &owner).await["status"],
        "active"
    );
    app.finish().await;
}
