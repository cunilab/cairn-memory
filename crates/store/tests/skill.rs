use cairn_store::StoreError;
use cairn_store::skill::{SkillDraft, SkillVote};
use cairn_store::testing::TestDb;

fn draft(body: &str) -> SkillDraft {
    SkillDraft {
        name: "e2e-postgres-cutover".into(),
        title: "E2E Postgres cutover".into(),
        description: "Rehearse a database migration".into(),
        body: body.into(),
        change_note: "E2E change".into(),
    }
}

#[tokio::test]
async fn versions_are_immutable_and_votes_are_per_person_and_version() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let alice = db.person("e2e-alice").await;
    let bob = db.person("e2e-bob").await;
    let store = &db.store;
    let first = store
        .save_skill(&alice, draft("Run the snapshot drill. token=abc123"), true)
        .await
        .unwrap();
    let id = &first.card.id;
    assert_eq!(first.card.version, 1);
    assert_eq!(first.author_id, alice);
    assert!(!first.body.contains("abc123"));
    assert!(matches!(
        store.save_skill(&bob, draft("Duplicate"), true).await,
        Err(StoreError::Conflict(_))
    ));

    let (second, third) = tokio::join!(
        store.save_skill(&bob, draft("New calibration"), false),
        store.save_skill(&bob, draft("Revised seeding"), false)
    );
    let mut versions = [second.unwrap(), third.unwrap()];
    versions.sort_by_key(|skill| skill.card.version);
    assert_eq!([versions[0].card.version, versions[1].card.version], [2, 3]);
    assert!(versions.iter().all(|skill| skill.author_id == bob));
    assert_eq!(
        store
            .get_skill(&bob, id, Some(1), false)
            .await
            .unwrap()
            .body,
        first.body
    );
    assert!(
        store
            .search_skills("snapshot", 5, 0, false)
            .await
            .unwrap()
            .results
            .is_empty()
    );
    let ranked = store.search_skills("migration", 1, 0, false).await.unwrap();
    assert_eq!(ranked.next_offset, None);
    let ranked = ranked.results;
    assert_eq!(ranked[0].id, *id);
    assert!(ranked[0].score > 0.0);
    assert!(
        store
            .search_skills("", 1, 1, false)
            .await
            .unwrap()
            .results
            .is_empty()
    );

    store
        .vote_for_skill(&alice, id, 1, SkillVote::Wrong, None)
        .await
        .unwrap();
    store
        .vote_for_skill(&bob, id, 1, SkillVote::Helpful, None)
        .await
        .unwrap();
    let vote = store
        .vote_for_skill(&alice, id, 1, SkillVote::Helpful, Some("token=abc123"))
        .await
        .unwrap();
    assert_eq!((vote.card.votes.helpful, vote.card.votes.wrong), (2, 0));
    assert!(!vote.my_note.unwrap().contains("abc123"));
    assert_eq!(
        store
            .get_skill(&alice, id, None, false)
            .await
            .unwrap()
            .card
            .votes
            .helpful,
        0
    );

    let rolled_back = store
        .roll_back_skill(&alice, id, 1, "E2E restore old steps")
        .await
        .unwrap();
    assert_eq!(rolled_back.card.version, 4);
    assert_eq!(rolled_back.body, first.body);
    assert_eq!(rolled_back.author_id, alice);
    assert_eq!(rolled_back.card.votes.helpful, 0);
    assert_eq!(
        store
            .skill_versions(id)
            .await
            .unwrap()
            .iter()
            .map(|v| v.version)
            .collect::<Vec<_>>(),
        [4, 3, 2, 1]
    );

    let valid_body = "é".repeat(16_000);
    assert_eq!(
        store
            .save_skill(&alice, draft(&valid_body), false)
            .await
            .unwrap()
            .body,
        valid_body
    );
    assert!(matches!(
        store
            .save_skill(&alice, draft(&(valid_body + "x")), false)
            .await,
        Err(StoreError::Validation(_))
    ));
    db.finish().await;
}
