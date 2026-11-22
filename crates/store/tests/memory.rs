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
        serde_json::json!({"kind":"fact","title":"Key title for tests","summary":"token=abc123 is used","reason":"r and the test says why"}),
    );
    let other = save_input(
        serde_json::json!({"kind":"fact","title":"Key title for tests","summary":"token=zzz999 is used","reason":"r and the test says why"}),
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
            save_input(serde_json::json!({"kind":"note","title":"n title for tests","summary":"s is a summary of the test"})),
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
            .is_none(),
        "notes do not expire"
    );
    assert!(
        db.store
            .save(
                &me,
                save_input(serde_json::json!({"kind":"decision","title":"d title for tests","summary":"s is a summary of the test"})),
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

    let preference = save_input(
        serde_json::json!({"kind":"preference","title":"p title for tests","summary":"s is a summary of the test","reason":"r and the test says why"}),
    );
    let id = db.store.save(&owner, preference, &work).await.unwrap().id;
    assert_eq!(
        db.store.get(&owner, &id).await.unwrap().scope,
        Scope::Personal,
        "preference is always personal"
    );
    let bad = save_input(
        serde_json::json!({"kind":"preference","title":"p2 title for tests","summary":"s is a summary of the test","reason":"r and the test says why","scope":"project"}),
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
        "SELECT title, status, (SELECT count(embedding) FROM memories), (SELECT count(*) FROM entities) FROM memories WHERE memory_id = $1",
    )
    .bind(&saved.id)
    .fetch_one(db.store.pool())
    .await
    .unwrap();
    assert_eq!(status.as_str(), "forgotten");
    assert!(!title.is_empty(), "forget keeps the title");
    assert_eq!(embeddings, 1, "forget keeps the embedding");
    assert_eq!(entities, 0);

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
            save_input(serde_json::json!({"kind":"note","title":"expiring note","summary":"s is a summary of the test"})),
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
        [
            "expiring note",
            "global rule",
            "private rule",
            "project rule"
        ],
        "an old expires_at does not hide the row"
    );
    assert!(store.get(&owner, &expiring.id).await.is_ok());

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
    assert_eq!(
        titles,
        [
            "rule 4 (test rule)",
            "rule 3 (test rule)",
            "rule 2 (test rule)",
            "rule 1 (test rule)",
            "rule 0 (test rule)"
        ]
    );
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
async fn a_near_identical_save_is_deduplicated_and_changed_text_is_not() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let save = |title: &str, summary: &str, reason: &str| {
        let input = save_input(
            serde_json::json!({"kind":"fact","title":title,"summary":summary,"reason":reason}),
        );
        let (store, me) = (db.store.clone(), me.clone());
        async move {
            store
                .save(&me, input, &WorkContext::default())
                .await
                .unwrap()
        }
    };
    let first = save(
        "Deploys run from the main branch",
        "Every deploy is cut from main after the checks pass.",
        "Agreed in the release review and written down here.",
    )
    .await;
    assert!(!first.deduplicated);
    // Same kind, title and summary: another reason, case or spacing is still the same memory.
    let again = save(
        "deploys  run from the Main branch",
        "Every deploy is cut from main after the checks pass.",
        "A different reason, written the second time this was saved.",
    )
    .await;
    assert_eq!(
        (again.deduplicated, again.id.as_str()),
        (true, first.id.as_str())
    );
    // Changed text is a new memory.
    let changed = save(
        "Deploys run from the main branch",
        "Every deploy is cut from main after the checks and a manual approval.",
        "Agreed in the release review and written down here.",
    )
    .await;
    assert!(!changed.deduplicated);
    assert_ne!(changed.id, first.id);
    db.finish().await;
}

#[tokio::test]
async fn a_helpful_vote_takes_back_the_same_persons_wrong_vote() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let owner = db.person("owner").await;
    let id = db
        .store
        .save(&owner, rule("rule flipped"), &WorkContext::default())
        .await
        .unwrap()
        .id;
    assert_eq!(
        db.store.feedback(&owner, &id, Vote::Wrong).await.unwrap(),
        Status::Stale
    );
    // The owner changes their mind: one vote per person, so the wrong is gone and it is active.
    assert_eq!(
        db.store.feedback(&owner, &id, Vote::Helpful).await.unwrap(),
        Status::Active
    );
    let memory = db.store.get(&owner, &id).await.unwrap();
    assert_eq!(
        (memory.status, memory.usage.helpful, memory.usage.wrong),
        (Status::Active, 1, 0)
    );
    // A memory that is stale for a missing file is not revived by a vote.
    sqlx::query(
        "UPDATE memories SET status = 'stale', stale_cause = 'missing' WHERE memory_id = $1",
    )
    .bind(&id)
    .execute(db.store.pool())
    .await
    .unwrap();
    assert_eq!(
        db.store.feedback(&owner, &id, Vote::Helpful).await.unwrap(),
        Status::Stale
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
        "kind": "note", "title": "Session note", "summary": "Looked at the pool. is a summary of the test", "reason": "context and the test says why"
    }));
    let work = WorkContext::default();
    let first = db.store.save(&me, note.clone(), &work).await.unwrap();
    sqlx::query("UPDATE memories SET expires_at = now() - interval '1 day' WHERE memory_id = $1")
        .bind(&first.id)
        .execute(db.store.pool())
        .await
        .unwrap();
    assert!(db.store.get(&me, &first.id).await.is_ok());
    let again = db.store.save(&me, note, &work).await.unwrap();
    assert!(again.deduplicated, "the same note stays one row");
    let memory = db.store.get(&me, &again.id).await.expect("still readable");
    assert!(memory.expires_at.unwrap() < cairn_core::id::now_seconds());
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
async fn a_correction_that_only_changes_the_reason_keeps_the_reason() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let work = WorkContext::default();
    let old = db.store.save(&me, rule("same rule"), &work).await.unwrap();
    let mut again = rule("same rule");
    again.reason = "the first reason was wrong".into();
    again.supersedes = Some(old.id.clone());
    let saved = db.store.save(&me, again, &work).await.unwrap();
    assert!(saved.deduplicated);
    assert_eq!(saved.id, old.id);
    let memory = db.store.get(&me, &old.id).await.unwrap();
    assert_eq!(memory.reason, "the first reason was wrong");
    assert_eq!(memory.status, Status::Active);
    assert!(memory.links.is_empty());
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

fn alias_input(title: &str, aliases: serde_json::Value) -> cairn_core::memory::SaveInput {
    serde_json::from_value(serde_json::json!({
        "kind": "fact",
        "title": title,
        "summary": "A summary long enough to pass the quality rule.",
        "reason": "because the test needs one",
        "aliases": aliases,
    }))
    .expect("aliases field")
}

/// Six aliases, or one of 121 characters, is rejected. Five valid ones are stored as agent rows.
/// A supersede does not copy the old memory's aliases.
#[tokio::test]
async fn alias_limits() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("me").await;
    let work = WorkContext::default();
    let six = alias_input(
        "Six aliases rejected",
        serde_json::json!(["a", "b", "c", "d", "e", "f"]),
    );
    let err = db.store.save(&me, six, &work).await.unwrap_err();
    assert!(
        matches!(err, StoreError::Validation(ref m) if m.contains("aliases")),
        "{err}"
    );
    let long = alias_input("Long alias rejected", serde_json::json!(["x".repeat(121)]));
    let err = db.store.save(&me, long, &work).await.unwrap_err();
    assert!(
        matches!(err, StoreError::Validation(ref m) if m.contains("alias")),
        "{err}"
    );
    let empty = alias_input("Empty alias rejected", serde_json::json!([""]));
    let err = db.store.save(&me, empty, &work).await.unwrap_err();
    assert!(matches!(err, StoreError::Validation(_)), "{err}");

    let aliases = vec![
        "one way to ask".to_owned(),
        "two way to ask".to_owned(),
        "three way to ask".to_owned(),
        "four way to ask".to_owned(),
        "y".repeat(120),
    ];
    let saved = db
        .store
        .save(
            &me,
            alias_input("Five aliases stored", serde_json::json!(aliases)),
            &work,
        )
        .await
        .unwrap();
    let rows: Vec<(i16, String, String)> = sqlx::query_as(
        "SELECT ord, source, text FROM memory_aliases WHERE memory_id = $1 ORDER BY ord",
    )
    .bind(&saved.id)
    .fetch_all(db.store.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 5);
    assert!(rows.iter().all(|(_, source, _)| source == "agent"));
    assert_eq!(
        rows.iter().map(|(ord, _, _)| *ord).collect::<Vec<_>>(),
        [0, 1, 2, 3, 4]
    );
    assert_eq!(rows[4].2.chars().count(), 120);

    let newer = db
        .store
        .save(
            &me,
            save_input(serde_json::json!({
                "kind": "fact",
                "title": "Replacement without aliases",
                "summary": "A new memory that does not copy the old aliases.",
                "reason": "because the test needs one",
                "supersedes": saved.id,
            })),
            &work,
        )
        .await
        .unwrap();
    let copied: i64 =
        sqlx::query_scalar("SELECT count(*) FROM memory_aliases WHERE memory_id = $1")
            .bind(&newer.id)
            .fetch_one(db.store.pool())
            .await
            .unwrap();
    assert_eq!(copied, 0, "a supersede copies nothing");
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM memory_aliases WHERE memory_id = $1")
        .bind(&saved.id)
        .fetch_one(db.store.pool())
        .await
        .unwrap();
    assert_eq!(kept, 5);
    db.finish().await;
}
