//! Activity data: the audit endpoint (paging, actor filter, titles) and every recorded action.

mod common;
use cairn_core::process::jobs::{JobEnd, JobKind};
use common::{App, PASSWORD};
use serde_json::{Value, json};

fn rule(title: &str) -> Value {
    json!({"kind": "rule", "title": title, "summary": format!("{title} is the rule here."), "reason": "the test needs a reason"})
}

async fn me(app: &App, token: &str) -> String {
    app.get("auth/me", token).await["person_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn sql(app: &App, query: &str) {
    sqlx::query(query)
        .execute(app.db.store.pool())
        .await
        .unwrap();
}

fn actions(events: &Value) -> Vec<String> {
    events
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["action"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn audit_is_paged_newest_first() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let person = me(&app, &admin).await;
    sql(&app, "DELETE FROM audit_events").await;
    for i in 0..5 {
        app.state
            .store
            .record_audit(
                &person,
                None,
                "device_registered",
                Some(&format!("dev_{i}")),
                json!({}),
            )
            .await
            .unwrap();
        sql(&app, &format!("UPDATE audit_events SET created_at = now() - interval '{} minutes' WHERE subject_id = 'dev_{i}'", i + 1)).await;
    }
    let first = app.get("auth/audit?limit=2", &admin).await;
    let subjects = |page: &Value| -> Vec<String> {
        page.as_array()
            .unwrap()
            .iter()
            .map(|e| e["subject_id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(subjects(&first), ["dev_0", "dev_1"]);
    let before = first[1]["created_at"].as_i64().unwrap();
    let second = app
        .get(&format!("auth/audit?limit=2&before={before}"), &admin)
        .await;
    assert_eq!(subjects(&second), ["dev_2", "dev_3"]);
    let before = second[1]["created_at"].as_i64().unwrap();
    let third = app
        .get(&format!("auth/audit?limit=2&before={before}"), &admin)
        .await;
    assert_eq!(subjects(&third), ["dev_4"]);
    let before = third[0]["created_at"].as_i64().unwrap();
    assert!(
        app.get(&format!("auth/audit?before={before}"), &admin)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
    // The limit is clamped to 1..=200 and defaults to 50.
    assert_eq!(
        app.get("auth/audit?limit=500", &admin)
            .await
            .as_array()
            .unwrap()
            .len(),
        5
    );
    assert_eq!(
        app.get("auth/audit?limit=0", &admin)
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // Someone else's audit is theirs alone.
    let other = app.member(&admin, "other").await;
    assert!(
        app.get("auth/audit", &other)
            .await
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["action"] == "login")
    );
    app.finish().await;
}

#[tokio::test]
async fn audit_before_id_pages_events_that_share_a_second() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let person = me(&app, &admin).await;
    sql(&app, "DELETE FROM audit_events").await;
    for i in 0..3 {
        app.state
            .store
            .record_audit(
                &person,
                None,
                "links_added",
                Some(&format!("s{i}")),
                json!({}),
            )
            .await
            .unwrap();
    }
    sql(
        &app,
        "UPDATE audit_events SET created_at = now() - interval '1 hour'",
    )
    .await;
    let first = app.get("auth/audit?limit=2", &admin).await;
    let (at, id) = (
        first[1]["created_at"].as_i64().unwrap(),
        first[1]["audit_id"].as_str().unwrap(),
    );
    let rest = app
        .get(
            &format!("auth/audit?limit=2&before={at}&before_id={id}"),
            &admin,
        )
        .await;
    assert_eq!(rest.as_array().unwrap().len(), 1, "{rest}");
    let mut seen: Vec<&str> = first
        .as_array()
        .unwrap()
        .iter()
        .chain(rest.as_array().unwrap())
        .map(|e| e["audit_id"].as_str().unwrap())
        .collect();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 3, "no row lost or repeated");
    app.finish().await;
}

#[tokio::test]
async fn audit_filters_by_actor_and_type() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let person = me(&app, &admin).await;
    sql(&app, "DELETE FROM audit_events").await;
    for action in [
        "device_registered",
        "links_added",
        "memories_expired",
        "login",
        "memory_merged",
    ] {
        app.state
            .store
            .record_audit(&person, None, action, None, json!({}))
            .await
            .unwrap();
    }
    let system = actions(&app.get("auth/audit?actor=system", &admin).await);
    assert_eq!(system.len(), 3, "{system:?}");
    assert!(
        system
            .iter()
            .all(|a| ["links_added", "memories_expired", "memory_merged"].contains(&a.as_str()))
    );
    let people = actions(&app.get("auth/audit?actor=person", &admin).await);
    assert_eq!(people.len(), 2);
    assert!(
        people.contains(&"login".to_owned()) && people.contains(&"device_registered".to_owned())
    );
    assert_eq!(
        actions(
            &app.get("auth/audit?actor=system&type=links_added", &admin)
                .await
        ),
        ["links_added"]
    );
    let two = actions(&app.get("auth/audit?type=links_added,login", &admin).await);
    assert_eq!(two.len(), 2);
    // A type outside the actor's set is empty, not an error.
    assert!(
        app.get("auth/audit?actor=person&type=links_added", &admin)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
    let (status, body) = app
        .send("GET", "auth/audit?actor=robot", json!(null), Some(&admin))
        .await;
    assert_eq!((status, body["code"].as_str()), (400, Some("validation")));
    app.finish().await;
}

#[tokio::test]
async fn audit_records_listed_actions() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let bob = app.member(&admin, "bob").await;
    let admin_device = app.device(&admin, "laptop").await;
    let bob_device = app.device(&bob, "bob-laptop").await;
    let project = app.project(&admin_device, "github.com/acme/widgets").await;
    app.post(
        "projects/init",
        json!({"project_id": project, "repository": "x"}),
        &bob_device,
    )
    .await;
    let forgotten = app.save(&admin_device, rule("Forget me via forget")).await;
    let forgotten_again = app.save(&admin_device, rule("Forget me again")).await;
    app.post("forget", json!({"id": forgotten}), &admin_device)
        .await;
    app.post("forget", json!({"id": forgotten_again}), &admin_device)
        .await;
    app.post(
        &format!("projects/{project}/rename"),
        json!({"name": "Widgets"}),
        &admin,
    )
    .await;
    let bob_id = me(&app, &bob).await;
    app.post(
        &format!("projects/{project}/ownership"),
        json!({"personId": bob_id}),
        &admin,
    )
    .await;
    let device_id = app.get("devices", &admin).await[0]["device_id"]
        .as_str()
        .unwrap()
        .to_owned();
    app.post("devices/rotate", json!({"deviceId": device_id}), &admin)
        .await;
    app.post("devices/revoke", json!({"deviceId": device_id}), &admin)
        .await;
    app.post(
        &format!("auth/accounts/{bob_id}/password"),
        json!({"password": "reset horse battery staple"}),
        &admin,
    )
    .await;
    let second = app
        .ok(
            "POST",
            "auth/login",
            json!({"email": "admin@example.test", "password": PASSWORD}),
            None,
        )
        .await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    app.post("auth/logout", json!({}), &second).await;
    app.post(
        "auth/password",
        json!({"current_password": PASSWORD, "new_password": "changed horse battery staple"}),
        &admin,
    )
    .await;
    let fresh = app
        .ok(
            "POST",
            "auth/login",
            json!({"email": "admin@example.test", "password": "changed horse battery staple"}),
            None,
        )
        .await["token"]
        .as_str()
        .unwrap()
        .to_owned();

    let events = app.get("auth/audit?limit=200", &fresh).await;
    let seen = actions(&events);
    for expected in [
        "login",
        "logout",
        "password_changed",
        "password_reset",
        "account_registered",
        "device_registered",
        "device_rotated",
        "device_revoked",
        "project_initialized",
        "project_renamed",
        "project_transferred",
        "memory_forgotten",
    ] {
        assert!(
            seen.iter().any(|a| a == expected),
            "{expected} missing from {seen:?}"
        );
    }
    assert_eq!(
        seen.iter().filter(|a| *a == "memory_forgotten").count(),
        2,
        "forget and delete both record"
    );
    let text = events.to_string();
    assert!(
        !text.contains(PASSWORD) && !text.contains("cairn-device-") && !text.contains("cairn-web-"),
        "no secret in metadata"
    );
    // Failed sign-ins leave no record.
    app.send(
        "POST",
        "auth/login",
        json!({"email": "admin@example.test", "password": "nope nope nope nope"}),
        None,
    )
    .await;
    let after = actions(&app.get("auth/audit?limit=200", &fresh).await);
    assert_eq!(
        after.iter().filter(|a| *a == "login").count(),
        seen.iter().filter(|a| *a == "login").count(),
        "a failed sign-in leaves no record"
    );
    app.finish().await;
}

#[tokio::test]
async fn audit_resolves_titles_and_device_names() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let other = app.member(&admin, "other").await;
    let device = app.device(&admin, "laptop-claude").await;
    let other_device = app.device(&other, "other-laptop").await;
    let mine = app.save(&device, rule("Visible merged rule")).await;
    let also = app.save(&device, rule("Second merged rule")).await;
    let secret = app.save(&other_device, rule("Somebody else rule")).await;
    let device_id = app.get("devices", &admin).await[0]["device_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let person = me(&app, &admin).await;
    app.state
        .store
        .record_audit(
            &person,
            None,
            "memory_merged",
            Some(&mine),
            json!({"into": mine, "merged": [also, secret]}),
        )
        .await
        .unwrap();
    app.state
        .store
        .record_audit(
            &person,
            Some(&device_id),
            "device_registered",
            Some(&device_id),
            json!({}),
        )
        .await
        .unwrap();
    let events = app.get("auth/audit?actor=system", &admin).await;
    let titles = &events[0]["titles"];
    assert_eq!(titles[&mine], "Visible merged rule");
    assert_eq!(titles[&also], "Second merged rule");
    assert!(
        titles.get(&secret).is_none() || titles[&secret].is_null(),
        "a hidden memory's title is never resolved: {titles}"
    );
    let device_events = app
        .get("auth/audit?actor=person&type=device_registered", &admin)
        .await;
    assert_eq!(device_events[0]["device_name"], "laptop-claude");
    app.finish().await;
}

#[tokio::test]
async fn link_job_records_links_added() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "d").await;
    let project = app.project(&device, "github.com/acme/widgets").await;
    let context = json!({"project_id": project, "repository": "github.com/acme/widgets", "files": ["src/billing/money.rs"]});
    let mut a = rule("Money is integer cents");
    a["context"] = context.clone();
    let mut b = rule("Never divide money by float");
    b["context"] = context;
    let first = app.save(&device, a).await;
    let second = app.save(&device, b).await;
    app.settle().await;
    let events = app
        .get("auth/audit?actor=system&type=links_added", &admin)
        .await;
    let events = events.as_array().unwrap();
    // Whichever memory's job runs first links the pair; the other finds it already linked.
    assert_eq!(events.len(), 1, "{events:?}");
    let meta = &events[0]["metadata"];
    assert_eq!(meta["count"], 1);
    assert_eq!(meta["entity"], "src/billing/money.rs");
    let subject = events[0]["subject_id"].as_str().unwrap();
    let other = if subject == first { &second } else { &first };
    assert!(subject == first || subject == second);
    assert_eq!(meta["ids"], json!([other]));
    assert!(
        events[0]["titles"][subject].is_string() && events[0]["titles"][other.as_str()].is_string()
    );
    assert!(events[0]["device_id"].is_null());
    app.finish().await;
}

#[tokio::test]
async fn distilled_session_records_session_distilled_with_device() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "laptop-claude").await;
    let device_id = app.get("devices", &admin).await[0]["device_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let context = json!({"repository": "r", "branch": "feat/refunds", "files": ["src/queue.rs"]});
    for (id, kind, content) in [
        (
            "e1",
            "assistant",
            "Decision: use SKIP LOCKED for the queue\nReason: one datastore",
        ),
        ("e2", "prompt", "TODO: the retry path is untested"),
    ] {
        app.post("observe", json!({"event_id": id, "session_id": "s1", "kind": kind, "content": content, "context": context}), &device).await;
    }
    app.post("session/end", json!({"session_id": "s1"}), &device)
        .await;
    app.settle().await;
    let events = app
        .get("auth/audit?actor=system&type=session_distilled", &admin)
        .await;
    let events = events.as_array().unwrap();
    assert_eq!(events.len(), 1, "{events:?}");
    let meta = &events[0]["metadata"];
    assert_eq!(meta["branch"], "feat/refunds");
    assert_eq!(meta["device_id"], device_id.as_str());
    assert!(meta["notes"].is_i64());
    let handoff = meta["handoff_id"].as_str().expect("a handoff was written");
    assert!(events[0]["titles"].get(handoff).is_some(), "{events:?}");
    assert_eq!(events[0]["device_name"], "laptop-claude");
    // The handoff memory knows which device wrote it.
    let record = app.get(&format!("memories/{handoff}"), &admin).await;
    assert_eq!(record["device_name"], "laptop-claude");
    app.finish().await;
}

#[tokio::test]
async fn sweep_records_memories_expired_once() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "d").await;
    let one = app.save(&device, rule("Expires first rule")).await;
    let two = app.save(&device, rule("Expires second rule")).await;
    app.save(&device, rule("Stays alive rule")).await;
    sql(&app, &format!("UPDATE memories SET expires_at = now() - interval '1 hour' WHERE memory_id IN ('{one}', '{two}')")).await;
    app.state.store.sweep().await.unwrap();
    app.state.store.sweep().await.unwrap();
    let events = app
        .get("auth/audit?actor=system&type=memories_expired", &admin)
        .await;
    let events = events.as_array().unwrap();
    assert!(
        events.is_empty(),
        "sweep does not record memories_expired: {events:?}"
    );
    assert_eq!(
        app.get(&format!("memories/{one}"), &admin).await["title"],
        "Expires first rule"
    );
    assert_eq!(
        app.get(&format!("memories/{two}"), &admin).await["title"],
        "Expires second rule"
    );
    app.finish().await;
}

#[tokio::test]
async fn quarantine_records_job_quarantined_for_the_owner() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let other = app.member(&admin, "other").await;
    let device = app.device(&admin, "d").await;
    let id = app.save(&device, rule("A rule whose job fails")).await;
    let store = &app.state.store;
    let jobs = store.claim_jobs(JobKind::Embed, 10).await.unwrap();
    let job = jobs.iter().find(|j| j.key == id).expect("the embed job");
    store
        .settle_job(
            &job.job_id,
            job.claim_id.as_deref().unwrap(),
            JobEnd::Quarantine("boom".into()),
        )
        .await
        .unwrap();
    let events = app
        .get("auth/audit?actor=system&type=job_quarantined", &admin)
        .await;
    let events = events.as_array().unwrap();
    assert_eq!(events.len(), 1, "{events:?}");
    let meta = &events[0]["metadata"];
    assert_eq!(
        (
            meta["kind"].as_str(),
            meta["attempts"].as_i64(),
            meta["error"].as_str()
        ),
        (Some("embed"), Some(1), Some("boom"))
    );
    assert!(
        app.get("auth/audit?actor=system", &other)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
    let stats = app.get("stats", &admin).await;
    assert_eq!(stats["jobs"]["quarantined"], 1);
    app.finish().await;
}

/// Claims the job of `key` (the memory's own embed job) and settles it.
async fn settle_job_of(app: &App, kind: JobKind, key: &str, end: JobEnd) {
    let store = &app.state.store;
    let jobs = store.claim_jobs(kind, 10).await.unwrap();
    let job = jobs.iter().find(|j| j.key == key).expect("the job of key");
    store
        .settle_job(&job.job_id, job.claim_id.as_deref().unwrap(), end)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_poison_job_is_reported_once_and_attention_clears_when_it_heals() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "d").await;
    let id = app
        .save(&device, rule("A rule whose job keeps failing"))
        .await;
    let store = &app.state.store;
    let reported = || async {
        app.get("auth/audit?actor=system&type=job_quarantined", &admin)
            .await
            .as_array()
            .unwrap()
            .len()
    };
    let quarantined = || async {
        app.get("stats/attention", &admin).await["jobs"]["quarantined"]
            .as_i64()
            .unwrap()
    };

    settle_job_of(&app, JobKind::Embed, &id, JobEnd::Quarantine("boom".into())).await;
    assert_eq!((reported().await, quarantined().await), (1, 1));

    // The daily retry fails again, twice: the person is not told again, and the job stays flagged.
    for _ in 0..2 {
        sql(
            &app,
            "UPDATE jobs SET finished_at = now() - interval '25 hours' WHERE status = 'quarantined'",
        )
        .await;
        store.sweep().await.unwrap();
        settle_job_of(
            &app,
            JobKind::Embed,
            &id,
            JobEnd::Retry("still boom".into()),
        )
        .await;
        assert_eq!((reported().await, quarantined().await), (1, 1));
    }

    // Once the retry works the flag clears.
    sql(
        &app,
        "UPDATE jobs SET finished_at = now() - interval '25 hours' WHERE status = 'quarantined'",
    )
    .await;
    store.sweep().await.unwrap();
    settle_job_of(&app, JobKind::Embed, &id, JobEnd::Succeeded).await;
    assert_eq!((reported().await, quarantined().await), (1, 0));

    // A later failure is a new quarantine and is reported; the old row never double counts.
    store.enqueue_job(JobKind::Embed, &id).await.unwrap();
    settle_job_of(
        &app,
        JobKind::Embed,
        &id,
        JobEnd::Quarantine("again".into()),
    )
    .await;
    assert_eq!((reported().await, quarantined().await), (2, 1));
    store.enqueue_job(JobKind::Embed, &id).await.unwrap();
    settle_job_of(
        &app,
        JobKind::Embed,
        &id,
        JobEnd::Quarantine("and again".into()),
    )
    .await;
    assert_eq!(quarantined().await, 1, "one flagged row per job key");
    app.finish().await;
}
