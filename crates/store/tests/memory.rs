use cairn_core::capture::WorkContext;
use cairn_core::memory::{Kind, Relation, Scope, Status};
use cairn_store::memory::{ListFilter, Vote};
use cairn_store::testing::{TestDb, rule, save_input};
use cairn_store::{Store, StoreError};

fn in_project(project_id: &str) -> WorkContext {
    WorkContext {
        project_id: Some(project_id.into()),
        ..WorkContext::default()
    }
}

#[tokio::test]
async fn save_get_and_dedupe_on_identity() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let work = WorkContext {
        repository: Some("github.com/acme/cairn".into()),
        branch: Some("main".into()),
        commit: Some("abc123".into()),
        ..WorkContext::default()
    };
    let input = save_input(serde_json::json!({
        "kind": "gotcha", "title": "Search path", "summary": "Set search_path per connection.",
        "reason": "Tests share one database; see crates/store/src/lib.rs for the pool.", "evidence": ["crates/store/src/lib.rs", "crates/store/src/lib.rs"],
        "fields": {"symptom": "relation not found", "cause": "wrong schema", "fix": "SET search_path"}
    }));
    let saved = db.store.save(&me, input.clone(), &work).await.unwrap();
    assert!(!saved.deduplicated);
    assert_eq!(saved.status, Status::Active);

    let memory = db.store.get(&me, &saved.id).await.unwrap();
    assert_eq!(
        (memory.kind, memory.scope),
        (Kind::Gotcha, Scope::Personal),
        "no project: personal"
    );
    assert_eq!(memory.title, "Search path");
    assert_eq!(memory.evidence, ["crates/store/src/lib.rs"]);
    assert_eq!(memory.evidence_count, 1);
    assert_eq!(memory.context.branch.as_deref(), Some("main"));
    assert_eq!(memory.fields.to_json()["fix"], "SET search_path");
    assert!(memory.expires_at.is_none());
    assert!(
        memory
            .entities
            .iter()
            .any(|e| e.name == "crates/store/src/lib.rs"),
        "entities are derived: {:?}",
        memory.entities
    );
    assert_eq!(memory.identity_hash.len(), 64);

    let again = db.store.save(&me, input, &work).await.unwrap();
    assert!(again.deduplicated);
    assert_eq!(again.id, saved.id);
    assert_eq!(
        db.store
            .list(&me, &ListFilter::default())
            .await
            .unwrap()
            .memories
            .len(),
        1
    );

    // Secrets never reach the row, and two saves that differ only in the secret collapse.
    let secret = save_input(
        serde_json::json!({"kind":"fact","title":"Key","summary":"token=abc123 is used","reason":"r"}),
    );
    let other = save_input(
        serde_json::json!({"kind":"fact","title":"Key","summary":"token=zzz999 is used","reason":"r"}),
    );
    let first = db.store.save(&me, secret, &work).await.unwrap();
    assert!(
        !db.store
            .get(&me, &first.id)
            .await
            .unwrap()
            .summary
            .contains("abc123")
    );
    assert!(db.store.save(&me, other, &work).await.unwrap().deduplicated);

    let note = db
        .store
        .save(
            &me,
            save_input(serde_json::json!({"kind":"note","title":"n","summary":"s"})),
            &work,
        )
        .await
        .unwrap();
    assert!(
        db.store
            .get(&me, &note.id)
            .await
            .unwrap()
            .expires_at
            .is_some(),
        "notes expire"
    );
    assert!(
        db.store
            .save(
                &me,
                save_input(serde_json::json!({"kind":"decision","title":"d","summary":"s"})),
                &work
            )
            .await
            .is_err()
    );
    db.finish().await;
}

#[tokio::test]
async fn project_scope_default_follows_membership() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let owner = db.person("owner").await;
    let (project, _) = db
        .store
        .init_project(&owner, "github.com/acme/cairn", None)
        .await
        .unwrap();
    let work = in_project(&project.project_id);
    let saved = db
        .store
        .save(&owner, rule("Rule one"), &work)
        .await
        .unwrap();
    let memory = db.store.get(&owner, &saved.id).await.unwrap();
    assert_eq!(
        (memory.scope, memory.project_id.as_deref()),
        (Scope::Project, Some(project.project_id.as_str()))
    );

    let preference =
        save_input(serde_json::json!({"kind":"preference","title":"p","summary":"s","reason":"r"}));
    let id = db.store.save(&owner, preference, &work).await.unwrap().id;
    assert_eq!(
        db.store.get(&owner, &id).await.unwrap().scope,
        Scope::Personal,
        "preference is always personal"
    );
    let bad = save_input(
        serde_json::json!({"kind":"preference","title":"p2","summary":"s","reason":"r","scope":"project"}),
    );
    assert!(matches!(
        db.store.save(&owner, bad, &work).await,
        Err(StoreError::Validation(_))
    ));

    let unknown = in_project("proj_nope");
    let personal = db
        .store
        .save(&owner, rule("Rule two"), &unknown)
        .await
        .unwrap();
    assert_eq!(
        db.store.get(&owner, &personal.id).await.unwrap().scope,
        Scope::Personal
    );
    db.finish().await;
}

#[tokio::test]
async fn forget_blanks_the_row_and_a_resave_is_409() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let work = WorkContext::default();
    let saved = db.store.save(&me, rule("Forget me"), &work).await.unwrap();
    db.store
        .upsert_embedding(&saved.id, "h", &unit(0))
        .await
        .unwrap();
    db.store.forget(&me, &saved.id).await.unwrap();

    assert!(matches!(
        db.store.get(&me, &saved.id).await,
        Err(StoreError::NotFound(_))
    ));
    let (title, status, embeddings, entities): (String, String, i64, i64) = sqlx::query_as(
        "SELECT title, status, (SELECT count(*) FROM memory_embeddings), (SELECT count(*) FROM entities) FROM memories WHERE memory_id = $1",
    )
    .bind(&saved.id)
    .fetch_one(db.store.pool())
    .await
    .unwrap();
    assert_eq!(
        (title.as_str(), status.as_str(), embeddings, entities),
        ("", "forgotten", 0, 0)
    );

    let error = db
        .store
        .save(&me, rule("Forget me"), &work)
        .await
        .expect_err("forgotten");
    assert!(
        matches!(&error, StoreError::Conflict(m) if m == "forgotten"),
        "{error}"
    );
    db.store
        .forget(&me, &saved.id)
        .await
        .expect("forget is idempotent");
    db.finish().await;
}

#[tokio::test]
async fn supersedes_and_contradicts_become_links_and_a_status_change() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let work = WorkContext::default();
    let old = db
        .store
        .save(&me, rule("Old rule"), &work)
        .await
        .unwrap()
        .id;
    let disputed = db
        .store
        .save(&me, rule("Disputed rule"), &work)
        .await
        .unwrap()
        .id;

    let mut newer = rule("New rule");
    newer.supersedes = Some(old.clone());
    let new_id = db.store.save(&me, newer, &work).await.unwrap().id;
    let mut challenger = rule("Challenger");
    challenger.contradicts = Some(disputed.clone());
    let challenger_id = db.store.save(&me, challenger, &work).await.unwrap().id;

    let old_memory = db.store.get(&me, &old).await.unwrap();
    assert_eq!(
        old_memory.status,
        Status::Superseded,
        "history stays readable"
    );
    assert!(
        old_memory
            .links
            .iter()
            .any(|l| l.relation == Relation::Supersedes
                && l.source == new_id
                && l.target == old
                && !l.auto)
    );
    let disputed_memory = db.store.get(&me, &disputed).await.unwrap();
    assert_eq!(
        disputed_memory.status,
        Status::Active,
        "contradicts changes no status"
    );
    assert!(
        disputed_memory
            .links
            .iter()
            .any(|l| l.relation == Relation::Contradicts && l.source == challenger_id)
    );
    assert_eq!(db.store.get(&me, &new_id).await.unwrap().links.len(), 1);

    let mut unknown = rule("Dangling");
    unknown.supersedes = Some("mem_missing".into());
    assert!(matches!(
        db.store.save(&me, unknown, &work).await,
        Err(StoreError::NotFound(_))
    ));
    let other = db.person("other").await;
    let mut foreign = rule("Foreign");
    foreign.supersedes = Some(new_id.clone());
    assert!(
        db.store.save(&other, foreign, &work).await.is_err(),
        "cannot supersede what you cannot see"
    );

    // Forgetting the newer memory brings the older one back.
    db.store.forget(&me, &new_id).await.unwrap();
    assert_eq!(
        db.store.get(&me, &old).await.unwrap().status,
        Status::Active
    );

    db.finish().await;
}

async fn visible_titles(store: &Store, person: &str) -> Vec<String> {
    let mut titles: Vec<String> = store
        .visible_memories(person)
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.title)
        .collect();
    titles.sort();
    titles
}

#[tokio::test]
async fn visibility_matrix() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let store = &db.store;
    let owner = db.person("owner").await;
    let member = db.person("member").await;
    let stranger = db.person("stranger").await;
    let (project, _) = store
        .init_project(&owner, "github.com/acme/cairn", None)
        .await
        .unwrap();
    store
        .join_project(&member, &project.project_id, None)
        .await
        .unwrap();
    let work = in_project(&project.project_id);

    store
        .save(&owner, rule("project rule"), &work)
        .await
        .unwrap();
    let mut private = rule("private rule");
    private.scope = Some(Scope::Personal);
    store.save(&owner, private, &work).await.unwrap();
    let mut global = rule("global rule");
    global.scope = Some(Scope::Global);
    store.save(&owner, global, &work).await.unwrap();
    let gone = store
        .save(&owner, rule("forgotten rule"), &work)
        .await
        .unwrap();
    store.forget(&owner, &gone.id).await.unwrap();
    let expiring = store
        .save(
            &owner,
            save_input(serde_json::json!({"kind":"note","title":"expiring note","summary":"s"})),
            &work,
        )
        .await
        .unwrap();

    assert_eq!(
        visible_titles(store, &owner).await,
        [
            "expiring note",
            "global rule",
            "private rule",
            "project rule"
        ]
    );
    assert_eq!(
        visible_titles(store, &member).await,
        ["global rule", "project rule"]
    );
    assert_eq!(visible_titles(store, &stranger).await, ["global rule"]);
    assert!(store.get(&stranger, &expiring.id).await.is_err());

    sqlx::query(
        "UPDATE memories SET expires_at = now() - interval '1 second' WHERE memory_id = $1",
    )
    .bind(&expiring.id)
    .execute(store.pool())
    .await
    .unwrap();
    assert_eq!(
        visible_titles(store, &owner).await,
        ["global rule", "private rule", "project rule"],
        "expired rows are hidden"
    );
    assert!(store.get(&owner, &expiring.id).await.is_err());

    // Links never reveal a memory the reader cannot see.
    let mut pointer = rule("member note");
    pointer.scope = Some(Scope::Personal);
    let project_rule = store
        .visible_memories(&member)
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.title == "project rule")
        .unwrap();
    pointer.contradicts = Some(project_rule.id.clone());
    let pointer_id = store.save(&member, pointer, &work).await.unwrap().id;
    let seen_by_owner = store.get(&owner, &project_rule.id).await.unwrap();
    assert!(
        seen_by_owner.links.iter().all(|l| l.source != pointer_id),
        "private memory stays hidden"
    );
    assert_eq!(
        store
            .get(&member, &project_rule.id)
            .await
            .unwrap()
            .links
            .len(),
        1
    );
    db.finish().await;
}

#[tokio::test]
async fn list_is_paged_newest_first() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let work = WorkContext::default();
    for n in 0..5 {
        db.store
            .save(&me, rule(&format!("rule {n}")), &work)
            .await
            .unwrap();
    }
    let mut filter = ListFilter {
        limit: Some(2),
        ..ListFilter::default()
    };
    let mut titles = Vec::new();
    let mut pages = 0;
    loop {
        let page = db.store.list(&me, &filter).await.unwrap();
        pages += 1;
        titles.extend(page.memories.iter().map(|m| m.title.clone()));
        match page.next_cursor {
            Some(cursor) => filter.cursor = Some(cursor),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(titles, ["rule 4", "rule 3", "rule 2", "rule 1", "rule 0"]);
    let only_notes = ListFilter {
        kind: Some(Kind::Note),
        ..ListFilter::default()
    };
    assert!(
        db.store
            .list(&me, &only_notes)
            .await
            .unwrap()
            .memories
            .is_empty()
    );
    assert!(
        db.store
            .list(
                &me,
                &ListFilter {
                    cursor: Some("garbage".into()),
                    ..ListFilter::default()
                }
            )
            .await
            .is_err()
    );
    db.finish().await;
}

#[tokio::test]
async fn feedback_moves_status_and_usage() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let owner = db.person("owner").await;
    let member = db.person("member").await;
    let (project, _) = db
        .store
        .init_project(&owner, "github.com/acme/cairn", None)
        .await
        .unwrap();
    db.store
        .join_project(&member, &project.project_id, None)
        .await
        .unwrap();
    let work = in_project(&project.project_id);
    let a = db
        .store
        .save(&owner, rule("rule a"), &work)
        .await
        .unwrap()
        .id;
    let b = db
        .store
        .save(&owner, rule("rule b"), &work)
        .await
        .unwrap()
        .id;
    let c = db
        .store
        .save(&owner, rule("rule c"), &work)
        .await
        .unwrap()
        .id;

    assert_eq!(
        db.store.feedback(&member, &a, Vote::Helpful).await.unwrap(),
        Status::Active
    );
    assert_eq!(db.store.get(&owner, &a).await.unwrap().usage.helpful, 1);
    // Votes count distinct voters: a member repeating a vote changes nothing, a second member's
    // `wrong` flips it.
    for _ in 0..3 {
        assert_eq!(
            db.store.feedback(&member, &a, Vote::Helpful).await.unwrap(),
            Status::Active
        );
        assert_eq!(
            db.store.feedback(&member, &b, Vote::Wrong).await.unwrap(),
            Status::Active
        );
    }
    let usage = db.store.get(&owner, &b).await.unwrap().usage;
    assert_eq!(usage.wrong, 1, "one voter, one wrong");
    assert_eq!(db.store.get(&owner, &a).await.unwrap().usage.helpful, 1);
    let second = db.person("second").await;
    db.store
        .join_project(&second, &project.project_id, None)
        .await
        .unwrap();
    assert_eq!(
        db.store.feedback(&second, &b, Vote::Wrong).await.unwrap(),
        Status::Stale
    );
    assert_eq!(db.store.get(&owner, &b).await.unwrap().usage.wrong, 2);
    // One `wrong` from the owner is enough.
    assert_eq!(
        db.store.feedback(&owner, &c, Vote::Wrong).await.unwrap(),
        Status::Stale
    );
    assert_eq!(
        db.store.get(&owner, &c).await.unwrap().status,
        Status::Stale
    );
    assert!(
        db.store
            .feedback(&db.person("stranger").await, &c, Vote::Wrong)
            .await
            .is_err()
    );

    db.store
        .mark_fetched(std::slice::from_ref(&a))
        .await
        .unwrap();
    assert_eq!(db.store.get(&owner, &a).await.unwrap().usage.fetched, 1);
    assert!(db.store.forget(&member, &a).await.is_err(), "owner only");
    db.finish().await;
}

fn unit(index: usize) -> Vec<f32> {
    let mut v = vec![0.0; 384];
    v[index] = 1.0;
    v
}

#[tokio::test]
async fn saving_an_expired_note_again_makes_it_live() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let note = save_input(serde_json::json!({
        "kind": "note", "title": "Session note", "summary": "Looked at the pool.", "reason": "context"
    }));
    let work = WorkContext::default();
    let first = db.store.save(&me, note.clone(), &work).await.unwrap();
    sqlx::query("UPDATE memories SET expires_at = now() - interval '1 day' WHERE memory_id = $1")
        .bind(&first.id)
        .execute(db.store.pool())
        .await
        .unwrap();
    assert!(matches!(
        db.store.get(&me, &first.id).await,
        Err(StoreError::NotFound(_))
    ));
    let again = db.store.save(&me, note, &work).await.unwrap();
    assert!(!again.deduplicated, "a new life, not a hidden duplicate");
    let memory = db.store.get(&me, &again.id).await.expect("live again");
    assert!(memory.expires_at.unwrap() > cairn_core::id::now_seconds());
    assert_eq!(
        db.store
            .list(&me, &ListFilter::default())
            .await
            .unwrap()
            .memories
            .len(),
        1
    );
    db.finish().await;
}

#[tokio::test]
async fn a_duplicate_save_still_adds_its_links() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let work = WorkContext::default();
    let old = db
        .store
        .save(&me, rule("old rule"), &work)
        .await
        .unwrap()
        .id;
    let new = db.store.save(&me, rule("new rule"), &work).await.unwrap();
    let mut again = rule("new rule");
    again.supersedes = Some(old.clone());
    let saved = db.store.save(&me, again, &work).await.unwrap();
    assert!(saved.deduplicated);
    assert_eq!(saved.id, new.id);
    assert_eq!(
        db.store.get(&me, &old).await.unwrap().status,
        Status::Superseded
    );
    let links = db.store.get(&me, &new.id).await.unwrap().links;
    assert!(
        links
            .iter()
            .any(|l| l.relation == Relation::Supersedes && l.target == old)
    );
    db.finish().await;
}

#[tokio::test]
async fn other_peoples_worktree_paths_are_not_returned() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let owner = db.person("owner").await;
    let member = db.person("member").await;
    let (project, _) = db
        .store
        .init_project(&owner, "github.com/acme/cairn", None)
        .await
        .unwrap();
    db.store
        .join_project(&member, &project.project_id, None)
        .await
        .unwrap();
    let work = WorkContext {
        project_id: Some(project.project_id.clone()),
        worktree: Some("/Users/owner/code/cairn".into()),
        ..WorkContext::default()
    };
    let id = db
        .store
        .save(&owner, rule("shared"), &work)
        .await
        .unwrap()
        .id;
    let mine = db.store.get(&owner, &id).await.unwrap();
    assert_eq!(
        mine.context.worktree.as_deref(),
        Some("/Users/owner/code/cairn")
    );
    assert!(
        db.store
            .get(&member, &id)
            .await
            .unwrap()
            .context
            .worktree
            .is_none()
    );
    let listed = db
        .store
        .list(&member, &ListFilter::default())
        .await
        .unwrap();
    assert!(listed.memories[0].context.worktree.is_none());
    db.finish().await;
}

#[tokio::test]
async fn a_malformed_cursor_is_a_validation_error() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    for cursor in ["not a time|mem_1", "2026-13-45T00:00:00Z|mem_1", "|"] {
        let result = db
            .store
            .list(
                &me,
                &ListFilter {
                    cursor: Some(cursor.into()),
                    ..ListFilter::default()
                },
            )
            .await;
        assert!(
            matches!(result, Err(StoreError::Validation(_))),
            "{cursor}: {result:?}"
        );
    }
    db.finish().await;
}
