use cairn_core::capture::WorkContext;
use cairn_store::testing::{TestDb, save_input};
use sqlx::Row;

async fn healthy_store(db: &TestDb) {
    let person = db.person("owner").await;
    let (project, _) = db
        .store
        .init_project(&person, "github.com/example/young", None)
        .await
        .unwrap();
    let context = WorkContext {
        project_id: Some(project.project_id),
        ..WorkContext::default()
    };
    let id = db.store.save(&person, save_input(serde_json::json!({"kind":"fact","title":"Young store fact","summary":"A recent useful memory","reason":"Test a healthy store"})), &context).await.unwrap().id;
    let vector = format!("[1,{}]", vec!["0"; 383].join(","));
    sqlx::query("UPDATE memories SET embedding=$2::vector,embedding_model=$3 WHERE memory_id=$1")
        .bind(id)
        .bind(vector)
        .bind(cairn_core::process::embed::model_label())
        .execute(db.store.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn young_store_quiet() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    healthy_store(&db).await;
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM fixes")
        .fetch_one(db.store.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    db.finish().await;
}

#[tokio::test]
async fn semantic_off_reports_once() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    healthy_store(&db).await;
    db.store.fixes_nightly(false, "0.1.9").await.unwrap();
    let rows = sqlx::query(
        "SELECT detector, subject_kind, subject_id, person_id, device_id, evidence, action FROM fixes",
    )
    .fetch_all(db.store.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<String, _>("detector"), "F8");
    assert_eq!(rows[0].get::<String, _>("subject_kind"), "server");
    for field in ["subject_id", "person_id", "device_id"] {
        assert_eq!(rows[0].get::<Option<String>, _>(field), None);
    }
    let evidence: serde_json::Value = rows[0].get("evidence");
    assert_eq!(evidence["semantic_off"], true);
    assert_eq!(rows[0].get::<Option<serde_json::Value>, _>("action"), None);
    db.finish().await;
}

#[tokio::test]
async fn faded_ranks_lower_and_fetch_clears() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let person = db.person("owner").await;
    let id = db.store.save(&person, save_input(serde_json::json!({"kind":"fact", "title":"Fade test record", "summary":"A complete summary for the test", "reason":"test evidence"})), &WorkContext::default()).await.unwrap().id;
    sqlx::query("UPDATE memories SET faded_at = now() WHERE memory_id = $1")
        .bind(&id)
        .execute(db.store.pool())
        .await
        .unwrap();
    let faded = db.store.get(&person, &id).await.unwrap();
    let mut plain = faded.clone();
    plain.faded_at = None;
    let context = WorkContext::default();
    assert!(
        cairn_core::retrieve::rank::adjust(&faded, &context, faded.updated_at)
            < cairn_core::retrieve::rank::adjust(&plain, &context, plain.updated_at)
    );
    db.store
        .mark_fetched(std::slice::from_ref(&id))
        .await
        .unwrap();
    assert!(db.store.get(&person, &id).await.unwrap().faded_at.is_none());
    db.finish().await;
}

async fn stale_fixture(db: &TestDb) -> (String, String) {
    let owner = db.person("owner").await;
    let id = db.store.save(&owner, save_input(serde_json::json!({"kind":"fact", "title":"Stale test memory", "summary":"A complete summary for stale testing", "reason":"test evidence"})), &WorkContext::default()).await.unwrap().id;
    sqlx::query("UPDATE memories SET status='stale', stale_cause='feedback', status_changed_at=now()-interval '8 days' WHERE memory_id=$1").bind(&id).execute(db.store.pool()).await.unwrap();
    db.store
        .mark_fetched(std::slice::from_ref(&id))
        .await
        .unwrap();
    (owner, id)
}

async fn next_night(db: &TestDb) {
    sqlx::query("UPDATE server_state SET value = '2000-01-01' WHERE key = 'fixes_last_run'")
        .execute(db.store.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn idempotent_night() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (_, id) = stale_fixture(&db).await;
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let rows = sqlx::query("SELECT count FROM fixes WHERE detector='F2' AND subject_id=$1")
        .bind(&id)
        .fetch_all(db.store.pool())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<i32, _>("count"), 1);
    next_night(&db).await;
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let rows = sqlx::query("SELECT count FROM fixes WHERE detector='F2' AND subject_id=$1")
        .bind(&id)
        .fetch_all(db.store.pool())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<i32, _>("count"), 2);
    db.finish().await;
}

#[tokio::test]
async fn ignore_reappears() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (_, id) = stale_fixture(&db).await;
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    sqlx::query("UPDATE fixes SET status='ignored', ignored_until=now()-interval '1 day' WHERE detector='F2' AND subject_id=$1").bind(&id).execute(db.store.pool()).await.unwrap();
    next_night(&db).await;
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fixes WHERE detector='F2' AND status='pending'")
            .fetch_one(db.store.pool())
            .await
            .unwrap();
    assert_eq!(
        count, 0,
        "cooldown alone must not reopen a persistent problem"
    );
    sqlx::query("UPDATE fixes SET ignored_until=now()+interval '1 day' WHERE detector='F2' AND subject_id=$1").bind(&id).execute(db.store.pool()).await.unwrap();
    sqlx::query("UPDATE memories SET status='active' WHERE memory_id=$1")
        .bind(&id)
        .execute(db.store.pool())
        .await
        .unwrap();
    next_night(&db).await;
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    sqlx::query("UPDATE memories SET status='stale' WHERE memory_id=$1")
        .bind(&id)
        .execute(db.store.pool())
        .await
        .unwrap();
    next_night(&db).await;
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fixes WHERE detector='F2' AND status='pending'")
            .fetch_one(db.store.pool())
            .await
            .unwrap();
    assert_eq!(count, 0, "a returning problem waits for the cooldown");
    sqlx::query("UPDATE fixes SET ignored_until=now()-interval '1 day' WHERE detector='F2' AND subject_id=$1").bind(&id).execute(db.store.pool()).await.unwrap();
    next_night(&db).await;
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let statuses: Vec<String> = sqlx::query_scalar(
        "SELECT status FROM fixes WHERE detector='F2' AND subject_id=$1 ORDER BY first_seen_at",
    )
    .bind(&id)
    .fetch_all(db.store.pool())
    .await
    .unwrap();
    assert_eq!(statuses.len(), 2);
    assert!(statuses.contains(&"ignored".to_owned()));
    assert!(statuses.contains(&"pending".to_owned()));
    db.finish().await;
}

#[tokio::test]
async fn guards() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (_, id) = stale_fixture(&db).await;
    sqlx::query("UPDATE memories SET title='token=private-value', created_at=now()-interval '2 hours' WHERE memory_id=$1").bind(&id).execute(db.store.pool()).await.unwrap();
    sqlx::query("UPDATE memories SET scope='global' WHERE memory_id=$1")
        .bind(&id)
        .execute(db.store.pool())
        .await
        .unwrap();
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM fixes")
        .fetch_one(db.store.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    sqlx::query("UPDATE memories SET scope='personal', origin='system' WHERE memory_id=$1")
        .bind(&id)
        .execute(db.store.pool())
        .await
        .unwrap();
    next_night(&db).await;
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM fixes")
        .fetch_one(db.store.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    sqlx::query("UPDATE memories SET origin='agent', status='superseded' WHERE memory_id=$1")
        .bind(&id)
        .execute(db.store.pool())
        .await
        .unwrap();
    next_night(&db).await;
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM fixes")
        .fetch_one(db.store.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    db.finish().await;
}

#[tokio::test]
async fn evidence_has_no_text() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (person, id) = stale_fixture(&db).await;
    sqlx::query("UPDATE memories SET summary='SECRET_MARKER token=secret-value', reason='COMMAND_MARKER rm /private/path' WHERE memory_id=$1").bind(&id).execute(db.store.pool()).await.unwrap();
    sqlx::query("INSERT INTO lookup_log (person_id,kind,query,result_ids,hit) VALUES ($1,'search','QUERY_MARKER',ARRAY[]::text[],false)")
        .bind(&person).execute(db.store.pool()).await.unwrap();
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let redaction: serde_json::Value =
        sqlx::query_scalar("SELECT evidence FROM fixes WHERE detector='F9' AND subject_id=$1")
            .bind(&id)
            .fetch_one(db.store.pool())
            .await
            .unwrap();
    assert_eq!(redaction["class"], "secret");
    assert_eq!(redaction["field"], "summary");
    let action: serde_json::Value =
        sqlx::query_scalar("SELECT action FROM fixes WHERE detector='F9' AND subject_id=$1")
            .bind(&id)
            .fetch_one(db.store.pool())
            .await
            .unwrap();
    assert_eq!(
        action,
        serde_json::json!({"type":"redact_secret", "memory_id":id, "fields":["summary"]})
    );
    let evidence: Vec<serde_json::Value> = sqlx::query_scalar("SELECT evidence FROM fixes")
        .fetch_all(db.store.pool())
        .await
        .unwrap();
    assert!(!evidence.is_empty());
    let text = serde_json::to_string(&evidence).unwrap();
    for private in [
        "SECRET_MARKER",
        "secret-value",
        "COMMAND_MARKER",
        "QUERY_MARKER",
        "/private/path",
    ] {
        assert!(!text.contains(private));
    }
    db.finish().await;
}

async fn user_table_counts(db: &TestDb) -> Vec<(String, i64)> {
    let tables: Vec<String> = sqlx::query_scalar("SELECT tablename FROM pg_tables WHERE schemaname=current_schema() AND tablename NOT IN ('fixes','server_state') ORDER BY tablename").fetch_all(db.store.pool()).await.unwrap();
    let mut counts = Vec::new();
    for table in tables {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM \"{}\"",
            table.replace('"', "\"\"")
        ))
        .fetch_one(db.store.pool())
        .await
        .unwrap();
        counts.push((table, count));
    }
    counts
}

#[tokio::test]
async fn only_fixes_written() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    stale_fixture(&db).await;
    // server_state is excluded: the coordinator authorized one nightly marker row.
    let before = user_table_counts(&db).await;
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    assert_eq!(user_table_counts(&db).await, before);
    db.finish().await;
}

#[tokio::test]
async fn switch_on() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (person, id) = stale_fixture(&db).await;
    for i in 0..2 {
        sqlx::query("INSERT INTO lookup_log (person_id,session_id,kind,query,result_ids,hit) VALUES ($1,$2,'search','missing phrase','{}',false)")
            .bind(&person).bind(format!("s{i}")).execute(db.store.pool()).await.unwrap();
        sqlx::query("INSERT INTO lookup_log (person_id,session_id,kind,query,result_ids,hit) VALUES ($1,$2,'search','other phrase',ARRAY[$3],true)")
            .bind(&person).bind(format!("s{i}")).bind(&id).execute(db.store.pool()).await.unwrap();
        sqlx::query("INSERT INTO lookup_log (person_id,session_id,kind,result_ids,hit) VALUES ($1,$2,'get',ARRAY[$3],true)")
            .bind(&person).bind(format!("s{i}")).bind(&id).execute(db.store.pool()).await.unwrap();
    }
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fixes WHERE detector IN ('F10','F11','F12')")
            .fetch_one(db.store.pool())
            .await
            .unwrap();
    assert_eq!(count, 0);
    db.finish().await;
}

#[tokio::test]
async fn f1_waits_for_seven_day_gate() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let person = db.person("owner").await;
    let (project, _) = db
        .store
        .init_project(&person, "github.com/example/gate", None)
        .await
        .unwrap();
    sqlx::query("INSERT INTO sessions (person_id,session_id,project_id,started_at,nudge_asked,nudge_saved,notes_written) VALUES ($1,'gate',$2,now()-interval '10 days',false,false,0)")
        .bind(&person).bind(&project.project_id).execute(db.store.pool()).await.unwrap();
    for i in 0..3 {
        let session = format!("working-{i}");
        sqlx::query("INSERT INTO sessions (person_id,session_id,project_id,started_at,nudge_asked,nudge_saved,notes_written) VALUES ($1,$2,$3,now()-interval '5 days',false,false,0)")
            .bind(&person).bind(&session).bind(&project.project_id).execute(db.store.pool()).await.unwrap();
        sqlx::query("INSERT INTO observations (person_id,event_id,session_id,role,source,kind,content,content_hash,expires_at) VALUES ($1,$2,$2,'assistant','tool','edit','changed','hash',now()+interval '1 day')")
            .bind(&person).bind(&session).execute(db.store.pool()).await.unwrap();
    }
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM fixes WHERE detector='F1'")
        .fetch_one(db.store.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    sqlx::query("UPDATE sessions SET started_at=now()-interval '15 days' WHERE person_id=$1 AND session_id='gate'").bind(&person).execute(db.store.pool()).await.unwrap();
    next_night(&db).await;
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM fixes WHERE detector='F1'")
        .fetch_one(db.store.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    db.finish().await;
}

#[tokio::test]
async fn aliases_off_suppresses_f11_after_window() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let (person, id) = stale_fixture(&db).await;
    let vector = format!("[1,{}]", vec!["0"; 383].join(","));
    sqlx::query("UPDATE memories SET embedding=$2::vector,embedding_model=$3 WHERE memory_id=$1")
        .bind(&id)
        .bind(vector)
        .bind(cairn_core::process::embed::model_label())
        .execute(db.store.pool())
        .await
        .unwrap();
    for i in 0..2 {
        let session = format!("alias-{i}");
        sqlx::query("INSERT INTO lookup_log (person_id,session_id,kind,query,result_ids,hit,at) VALUES ($1,$2,'search','alias phrase','{}',false,now()-interval '15 days')")
            .bind(&person).bind(&session).execute(db.store.pool()).await.unwrap();
        sqlx::query("INSERT INTO lookup_log (person_id,session_id,kind,query,result_ids,hit,at) VALUES ($1,$2,'search','other',ARRAY[$3],true,now()-interval '14 days')")
            .bind(&person).bind(&session).bind(&id).execute(db.store.pool()).await.unwrap();
        sqlx::query("INSERT INTO lookup_log (person_id,session_id,kind,result_ids,hit,at) VALUES ($1,$2,'get',ARRAY[$3],true,now()-interval '13 days')")
            .bind(&person).bind(&session).bind(&id).execute(db.store.pool()).await.unwrap();
    }
    let mut embedding = vec![0.0; 384];
    embedding[0] = 1.0;
    db.store
        .fixes_nightly_with_queries(true, "0.1.9", &[("alias phrase".into(), embedding)])
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM fixes WHERE detector='F11'")
        .fetch_one(db.store.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    db.finish().await;
}

#[tokio::test]
async fn f10_f12_wait_for_their_data_windows() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let person = db.person("owner").await;
    let (project, _) = db
        .store
        .init_project(&person, "github.com/example/windows", None)
        .await
        .unwrap();
    let context = WorkContext {
        project_id: Some(project.project_id.clone()),
        ..WorkContext::default()
    };
    let id = db.store.save(&person, save_input(serde_json::json!({"kind":"fact","title":"Old work note","summary":"A complete factual summary","reason":"Test age gate"})), &context).await.unwrap().id;
    sqlx::query("UPDATE memories SET created_at=now()-interval '89 days' WHERE memory_id=$1")
        .bind(&id)
        .execute(db.store.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO sessions (person_id,session_id,project_id,started_at,nudge_asked,nudge_saved) VALUES ($1,'earliest',$2,now()-interval '13 days',true,false)")
        .bind(&person).bind(&project.project_id).execute(db.store.pool()).await.unwrap();
    for i in 0..10 {
        let session = format!("asked-{i}");
        sqlx::query("INSERT INTO sessions (person_id,session_id,project_id,started_at,nudge_asked,nudge_saved) VALUES ($1,$2,$3,now()-interval '1 day',true,false)")
            .bind(&person).bind(&session).bind(&project.project_id).execute(db.store.pool()).await.unwrap();
        for _ in 0..2 {
            sqlx::query("INSERT INTO lookup_log (person_id,session_id,kind,query,hit) VALUES ($1,$2,'search','ordinary',false)")
                .bind(&person).bind(&session).execute(db.store.pool()).await.unwrap();
        }
    }
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM fixes WHERE detector IN ('F10','F12')")
            .fetch_one(db.store.pool())
            .await
            .unwrap();
    assert_eq!(count, 0);
    sqlx::query("UPDATE sessions SET started_at=now()-interval '15 days' WHERE person_id=$1 AND session_id='earliest'").bind(&person).execute(db.store.pool()).await.unwrap();
    sqlx::query("UPDATE memories SET created_at=now()-interval '91 days' WHERE memory_id=$1")
        .bind(&id)
        .execute(db.store.pool())
        .await
        .unwrap();
    next_night(&db).await;
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let detectors: Vec<String> = sqlx::query_scalar(
        "SELECT detector FROM fixes WHERE detector IN ('F10','F12') ORDER BY detector",
    )
    .fetch_all(db.store.pool())
    .await
    .unwrap();
    assert_eq!(detectors, vec!["F10", "F12"]);
    db.finish().await;
}

#[tokio::test]
async fn agent_proof_requires_one_new_successor() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let person = db.person("owner").await;
    let context = WorkContext::default();
    let old = db.store.save(&person, save_input(serde_json::json!({"kind":"fact","title":"Old proof record","summary":"The original test fact","reason":"Old evidence"})), &context).await.unwrap().id;
    let new = db.store.save(&person, save_input(serde_json::json!({"kind":"fact","title":"New proof record","summary":"A corrected test fact","reason":"New evidence"})), &context).await.unwrap().id;
    sqlx::query("INSERT INTO links (source_id,target_id,relation,origin) VALUES ($1,$2,'supersedes','agent')").bind(&new).bind(&old).execute(db.store.pool()).await.unwrap();
    sqlx::query("INSERT INTO fixes (fix_id,detector,fingerprint,subject_kind,subject_id,person_id,fix_type,status,first_seen_at,last_seen_at,waiting_since) VALUES ('proof','F2','proof','memory',$1,$2,'agent','waiting',now(),now(),now()-interval '1 minute')")
        .bind(&old).bind(&person).execute(db.store.pool()).await.unwrap();
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let row = sqlx::query("SELECT status,proof_id FROM fixes WHERE fix_id='proof'")
        .fetch_one(db.store.pool())
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("status"), "done");
    assert_eq!(row.get::<String, _>("proof_id"), new);
    let other = db.store.save(&person, save_input(serde_json::json!({"kind":"fact","title":"Second proof record","summary":"Another corrected test fact","reason":"Other evidence"})), &context).await.unwrap().id;
    sqlx::query("INSERT INTO links (source_id,target_id,relation,origin) VALUES ($1,$2,'supersedes','agent')").bind(&other).bind(&old).execute(db.store.pool()).await.unwrap();
    sqlx::query(
        "UPDATE fixes SET status='waiting',proof_id=NULL,done_at=NULL WHERE fix_id='proof'",
    )
    .execute(db.store.pool())
    .await
    .unwrap();
    next_night(&db).await;
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM fixes WHERE fix_id='proof'")
        .fetch_one(db.store.pool())
        .await
        .unwrap();
    assert_eq!(status, "waiting");
    db.finish().await;
}

#[tokio::test]
async fn f6_counts_sessions_per_device_and_reports_ambiguous_projects() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let person = db.person("owner").await;
    let other = db.person("other").await;
    let (project, _) = db
        .store
        .init_project(&other, "github.com/example/shared", None)
        .await
        .unwrap();
    for id in ["d1", "d2"] {
        sqlx::query(
            "INSERT INTO devices (device_id,person_id,name,token_hash) VALUES ($1,$2,$1,$1)",
        )
        .bind(id)
        .bind(&person)
        .execute(db.store.pool())
        .await
        .unwrap();
    }
    for (id, device) in [("s1", "d1"), ("s2", "d1"), ("s3", "d2")] {
        sqlx::query(
            "INSERT INTO sessions (person_id,session_id,device_id,context) VALUES ($1,$2,$3,$4)",
        )
        .bind(&person)
        .bind(id)
        .bind(device)
        .bind(serde_json::json!({"repository":"github.com/example/shared"}))
        .execute(db.store.pool())
        .await
        .unwrap();
    }
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let rows = sqlx::query(
        "SELECT fix_type,device_id,project_id,action FROM fixes WHERE detector='F6' AND status='pending'",
    )
    .fetch_all(db.store.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<String, _>("fix_type"), "computer");
    assert_eq!(rows[0].get::<String, _>("device_id"), "d1");
    assert_eq!(rows[0].get::<String, _>("project_id"), project.project_id);
    let action: serde_json::Value = rows[0].get("action");
    assert_eq!(action["name"], project.name);
    db.store
        .init_project(&person, "github.com/example/shared", None)
        .await
        .unwrap();
    next_night(&db).await;
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let rows = sqlx::query(
        "SELECT fix_type,device_id,action FROM fixes WHERE detector='F6' AND status='pending'",
    )
    .fetch_all(db.store.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<String, _>("fix_type"), "report");
    assert_eq!(rows[0].get::<Option<String>, _>("device_id"), None);
    assert_eq!(rows[0].get::<Option<serde_json::Value>, _>("action"), None);
    db.finish().await;
}

#[tokio::test]
async fn f7_finds_historical_auto_merge_with_rare_token_mismatch() {
    let Some(db) = TestDb::new().await else {
        return;
    };
    let person = db.person("owner").await;
    let context = WorkContext::default();
    let old = db.store.save(&person, save_input(serde_json::json!({"kind":"fact","title":"Use node 20","summary":"The older supported runtime","reason":"Older evidence"})), &context).await.unwrap().id;
    let new = db.store.save(&person, save_input(serde_json::json!({"kind":"fact","title":"Use node 22","summary":"The newer supported runtime","reason":"New evidence"})), &context).await.unwrap().id;
    sqlx::query("UPDATE memories SET status='superseded' WHERE memory_id=$1")
        .bind(&old)
        .execute(db.store.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO links (source_id,target_id,relation,origin) VALUES ($1,$2,'supersedes','auto')").bind(&new).bind(&old).execute(db.store.pool()).await.unwrap();
    db.store.fixes_nightly(true, "0.1.9").await.unwrap();
    let row = sqlx::query("SELECT subject_id,action FROM fixes WHERE detector='F7'")
        .fetch_one(db.store.pool())
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("subject_id"), new);
    let action: serde_json::Value = row.get("action");
    assert_eq!(action["type"], "split_merge");
    assert_eq!(action["target_id"], old);
    db.finish().await;
}
