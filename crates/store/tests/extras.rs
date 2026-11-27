//! Session provenance, skill source visibility, and pin order.

use cairn_core::capture::WorkContext;
use cairn_core::memory::Scope;
use cairn_core::retrieve::brief::BriefBudget;
use cairn_store::StoreError;
use cairn_store::memory::{PinOrder, PinSlot};
use cairn_store::skill::SkillDraft;
use cairn_store::testing::{TestDb, rule};
use sqlx::Row;

fn project_work(project_id: &str) -> WorkContext {
    WorkContext {
        project_id: Some(project_id.to_owned()),
        repository: Some("github.com/acme/cairn".to_owned()),
        branch: Some("main".to_owned()),
        ..WorkContext::default()
    }
}

async fn positions(store: &cairn_store::Store, scope: &str, bucket: &str) -> Vec<PinSlot> {
    let rows = sqlx::query(
        "SELECT memory_id, pin_position FROM memories
          WHERE pinned_at IS NOT NULL AND scope = $1
            AND CASE WHEN $1 = 'project' THEN project_id = $2 ELSE owner_id = $2 END
          ORDER BY pin_position, memory_id",
    )
    .bind(scope)
    .bind(bucket)
    .fetch_all(store.pool())
    .await
    .unwrap();
    rows.iter()
        .map(|row| PinSlot {
            id: row.get("memory_id"),
            position: row.get("pin_position"),
        })
        .collect()
}

#[tokio::test]
async fn provenance_is_written_only_for_a_new_save_in_a_real_session() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("prov").await;
    let other = db.person("other").await;
    db.store
        .capture(
            &me,
            serde_json::from_value(serde_json::json!({
                "event_id": "e1", "session_id": "s1", "content": "hello", "kind": "prompt",
                "context": {"repository": "github.com/acme/cairn", "branch": "main"}
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let work = WorkContext::default();
    let input = rule("Provenance rule one");
    let saved = db
        .store
        .save_from_session(&me, input.clone(), &work, None, Some("s1"))
        .await
        .unwrap();
    assert!(!saved.deduplicated);
    let again = db
        .store
        .save_from_session(&me, input, &work, None, Some("s1"))
        .await
        .unwrap();
    assert!(again.deduplicated);
    let missing = db
        .store
        .save_from_session(
            &me,
            rule("Provenance rule two"),
            &work,
            None,
            Some("no-such-session"),
        )
        .await
        .unwrap();
    let links: i64 = sqlx::query_scalar("SELECT count(*) FROM memory_sessions")
        .fetch_one(db.store.pool())
        .await
        .unwrap();
    assert_eq!(links, 1);
    let views = db
        .store
        .memory_views(&me, vec![db.store.get(&me, &saved.id).await.unwrap()])
        .await
        .unwrap();
    let provenance = views[0].provenance.as_ref().unwrap();
    assert_eq!(provenance.session_id, "s1");
    assert_eq!(provenance.role, "saved");
    assert_eq!(provenance.branch.as_deref(), Some("main"));
    let hidden = db
        .store
        .memory_views(&other, vec![db.store.get(&me, &saved.id).await.unwrap()])
        .await
        .unwrap();
    assert!(
        hidden[0].provenance.is_none(),
        "another person cannot see this personal session"
    );
    assert!(db.store.get(&other, &missing.id).await.is_err());
    db.finish().await;
}

#[tokio::test]
async fn project_provenance_hides_a_chat_session_from_members() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let owner = db.person("owner").await;
    let member = db.person("member").await;
    let (project, _) = db
        .store
        .init_project(&owner, "local:prov", Some("Prov"))
        .await
        .unwrap();
    let project_id = project.project_id;
    sqlx::query("INSERT INTO project_members (project_id, person_id) VALUES ($1, $2)")
        .bind(&project_id)
        .bind(&member)
        .execute(db.store.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO sessions (person_id, session_id, project_id, context) VALUES
         ($1, 'plain', $2, '{\"branch\":\"main\"}'),
         ($1, 'chat', $2, '{\"branch\":\"main\",\"chat\":{\"platform\":\"matrix\"}}')",
    )
    .bind(&owner)
    .bind(&project_id)
    .execute(db.store.pool())
    .await
    .unwrap();
    let work = project_work(&project_id);
    let plain = db
        .store
        .save_from_session(
            &owner,
            rule("Plain session rule"),
            &work,
            None,
            Some("plain"),
        )
        .await
        .unwrap();
    let chat = db
        .store
        .save_from_session(&owner, rule("Chat session rule"), &work, None, Some("chat"))
        .await
        .unwrap();
    let mine = db
        .store
        .memory_views(
            &owner,
            vec![
                db.store.get(&owner, &plain.id).await.unwrap(),
                db.store.get(&owner, &chat.id).await.unwrap(),
            ],
        )
        .await
        .unwrap();
    assert!(mine.iter().all(|view| view.provenance.is_some()));
    let shared = db
        .store
        .memory_views(
            &member,
            vec![
                db.store.get(&member, &plain.id).await.unwrap(),
                db.store.get(&member, &chat.id).await.unwrap(),
            ],
        )
        .await
        .unwrap();
    assert_eq!(shared[0].provenance.as_ref().unwrap().session_id, "plain");
    assert!(
        shared[1].provenance.is_none(),
        "chat session stays with its owner"
    );
    let inspection = db
        .store
        .inspect_session(&member, &owner, "plain")
        .await
        .unwrap();
    assert_eq!(inspection.outputs.len(), 1);
    assert!(
        db.store
            .inspect_session(&member, &owner, "chat")
            .await
            .is_err()
    );
    assert!(
        db.store
            .inspect_session(&owner, &owner, "chat")
            .await
            .unwrap()
            .outputs
            .len()
            == 1
    );
    db.finish().await;
}

#[tokio::test]
async fn distill_links_only_the_memories_it_creates() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let me = db.person("distill").await;
    db.store
        .capture(
            &me,
            serde_json::from_value(serde_json::json!({
                "event_id": "e1", "session_id": "s1", "kind": "prompt",
                "content": "Decision: keep the link\nReason: the session created it\nTODO: stop here",
                "context": {"branch": "main"}
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    db.store.end_session(&me, "s1").await.unwrap();
    let report = db.store.distill_session(&me, "s1").await.unwrap();
    assert!(report.notes >= 1 && report.handoff);
    let links: i64 =
        sqlx::query_scalar("SELECT count(*) FROM memory_sessions WHERE role = 'distilled'")
            .fetch_one(db.store.pool())
            .await
            .unwrap();
    let created = (report.notes + usize::from(report.handoff)) as i64;
    assert_eq!(links, created);
    db.store.distill_session(&me, "s1").await.unwrap();
    let again: i64 = sqlx::query_scalar("SELECT count(*) FROM memory_sessions")
        .fetch_one(db.store.pool())
        .await
        .unwrap();
    assert_eq!(again, links, "a second distill does not add links");
    db.finish().await;
}

#[tokio::test]
async fn skill_source_is_hidden_when_the_memory_is_hidden() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let owner = db.person("skill-owner").await;
    let member = db.person("skill-member").await;
    let stranger = db.person("skill-stranger").await;
    let (project, _) = db
        .store
        .init_project(&owner, "local:skill-src", Some("Skill source"))
        .await
        .unwrap();
    sqlx::query("INSERT INTO project_members (project_id, person_id) VALUES ($1, $2)")
        .bind(&project.project_id)
        .bind(&member)
        .execute(db.store.pool())
        .await
        .unwrap();
    let memory = db
        .store
        .save(
            &owner,
            rule("Source procedure rule"),
            &project_work(&project.project_id),
        )
        .await
        .unwrap();
    let skill = db
        .store
        .save_skill(
            &owner,
            SkillDraft {
                name: "source-link".into(),
                title: "Source link".into(),
                description: "Shows where a skill came from.".into(),
                body: "Do the thing.".into(),
                change_note: "first".into(),
            },
            true,
        )
        .await
        .unwrap();
    sqlx::query("UPDATE skills SET source_memory_id = $1 WHERE skill_id = $2")
        .bind(&memory.id)
        .bind(&skill.card.id)
        .execute(db.store.pool())
        .await
        .unwrap();
    let seen = db
        .store
        .get_skill(&member, &skill.card.id, None, false)
        .await
        .unwrap();
    let source = seen.source.unwrap();
    assert_eq!(source.title, "Source procedure rule");
    assert_eq!(
        source.project_id.as_deref(),
        Some(project.project_id.as_str())
    );
    assert_eq!(source.project_name.as_deref(), Some("Skill source"));
    let hidden = db
        .store
        .get_skill(&stranger, &skill.card.id, None, false)
        .await
        .unwrap();
    assert!(hidden.source.is_none());
    db.finish().await;
}

#[tokio::test]
async fn project_pin_permission_order_and_conflict() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let admin = db.person("pin-admin").await;
    let owner = db.person("pin-owner").await;
    let member = db.person("pin-member").await;
    let stranger = db.person("pin-stranger").await;
    let (project, _) = db
        .store
        .init_project(&owner, "local:pins", Some("Pins"))
        .await
        .unwrap();
    let project_id = project.project_id.clone();
    for person in [&member, &stranger] {
        sqlx::query("INSERT INTO project_members (project_id, person_id) VALUES ($1, $2)")
            .bind(&project_id)
            .bind(person)
            .execute(db.store.pool())
            .await
            .unwrap();
    }
    let work = project_work(&project_id);
    let first = db
        .store
        .save(&member, rule("First project pin"), &work)
        .await
        .unwrap()
        .id;
    let second = db
        .store
        .save(&owner, rule("Second project pin"), &work)
        .await
        .unwrap()
        .id;
    assert!(matches!(
        db.store.set_pinned(&stranger, &first, true).await,
        Err(StoreError::NotFound(_))
    ));
    assert!(
        db.store
            .set_pinned(&member, &first, true)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        db.store
            .set_pinned(&owner, &second, true)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        db.store
            .set_pinned(&admin, &first, true)
            .await
            .unwrap()
            .is_some()
    );
    let before = positions(&db.store, "project", &project_id).await;
    assert_eq!(before.len(), 2);
    assert!(matches!(
        db.store
            .reorder_pins(
                &stranger,
                Scope::Project,
                Some(&project_id),
                &pairs(&before)
            )
            .await,
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        db.store
            .reorder_pins(&member, Scope::Project, Some(&project_id), &pairs(&before))
            .await,
        Err(StoreError::NotFound(_))
    ));
    let swapped = vec![
        (before[1].id.clone(), before[1].position),
        (before[0].id.clone(), before[0].position),
    ];
    let applied = db
        .store
        .reorder_pins(&owner, Scope::Project, Some(&project_id), &swapped)
        .await
        .unwrap();
    let PinOrder::Applied(order) = applied else {
        panic!("expected the new order")
    };
    assert_eq!(order[0].id, before[1].id);
    assert_eq!(order[0].position, 1);
    let stale = db
        .store
        .reorder_pins(&admin, Scope::Project, Some(&project_id), &swapped)
        .await
        .unwrap();
    assert!(matches!(stale, PinOrder::Stale(_)));
    let current = positions(&db.store, "project", &project_id).await;
    let snapshot: Vec<(String, i32)> = current
        .iter()
        .map(|slot| (slot.id.clone(), slot.position))
        .collect();
    let reversed: Vec<(String, i32)> = snapshot.iter().rev().cloned().collect();
    let store = db.store.clone();
    let bucket = project_id.clone();
    let owner_id = owner.clone();
    let admin_id = admin.clone();
    let left = {
        let store = store.clone();
        let bucket = bucket.clone();
        let reversed = reversed.clone();
        tokio::spawn(async move {
            store
                .reorder_pins(&owner_id, Scope::Project, Some(&bucket), &reversed)
                .await
        })
    };
    let right = tokio::spawn(async move {
        store
            .reorder_pins(&admin_id, Scope::Project, Some(&bucket), &reversed)
            .await
    });
    let (a, b) = tokio::join!(left, right);
    let results = [a.unwrap().unwrap(), b.unwrap().unwrap()];
    assert!(
        results
            .iter()
            .any(|result| matches!(result, PinOrder::Applied(_)))
    );
    assert!(
        results
            .iter()
            .any(|result| matches!(result, PinOrder::Stale(_)))
    );
    let older = db
        .store
        .save(&member, rule("Older personal pin"), &WorkContext::default())
        .await
        .unwrap()
        .id;
    let newer = db
        .store
        .save(&member, rule("Newer personal pin"), &WorkContext::default())
        .await
        .unwrap()
        .id;
    db.store.set_pinned(&member, &older, true).await.unwrap();
    db.store.set_pinned(&member, &newer, true).await.unwrap();
    let personal = positions(&db.store, "personal", &member).await;
    let swapped_personal = vec![
        (personal[1].id.clone(), personal[1].position),
        (personal[0].id.clone(), personal[0].position),
    ];
    db.store
        .reorder_pins(&member, Scope::Personal, None, &swapped_personal)
        .await
        .unwrap();
    let brief = db
        .store
        .brief(
            &member,
            &WorkContext::default(),
            None,
            BriefBudget::Extended,
        )
        .await
        .unwrap();
    let l0 = brief.text.split("\"L1 Essential\"").next().unwrap();
    let newer_at = l0.find(&newer).unwrap();
    let older_at = l0.find(&older).unwrap();
    assert!(
        newer_at < older_at,
        "L0 follows pin_position, not pin time: {l0}"
    );
    db.finish().await;
}

fn pairs(slots: &[PinSlot]) -> Vec<(String, i32)> {
    slots
        .iter()
        .map(|slot| (slot.id.clone(), slot.position))
        .collect()
}
