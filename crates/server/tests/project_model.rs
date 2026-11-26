//! The shared project model over HTTP: accounts, devices, membership and who sees what.

mod common;
use common::{App, PASSWORD, ids};
use serde_json::json;

#[tokio::test]
async fn shared_project_model() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let author = app.member(&admin, "author").await;
    let stranger = app.member(&admin, "stranger").await;
    let author_device = app.device(&author, "author-laptop").await;
    let stranger_device = app.device(&stranger, "stranger-laptop").await;

    // A device token cannot manage accounts or devices; a stranger bearer is a plain 401.
    let (status, body) = app
        .send("GET", "devices", json!(null), Some(&author_device))
        .await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["code"], "session_required");
    let (status, _) = app.send("GET", "devices", json!(null), Some("nope")).await;
    assert_eq!(status, 401);
    let (status, _) = app
        .send("GET", "auth/accounts", json!(null), Some(&author))
        .await;
    assert_eq!(status, 403);

    // Init joins by committed id and returns the authoritative name even when the file is stale.
    let project = app
        .post(
            "projects/init",
            json!({"repository": "github.com/acme/widgets", "name": "Widgets"}),
            &author_device,
        )
        .await;
    assert_eq!(project["created"], true);
    let id = project["project_id"].as_str().unwrap();
    let joined = app
        .post(
            "projects/init",
            json!({"project_id": id, "repository": "x", "name": "Widgets"}),
            &stranger_device,
        )
        .await;
    assert_eq!(joined["created"], false);
    let joined_with_old_name = app
        .post(
            "projects/init",
            json!({"project_id": id, "repository": "x", "name": "Other"}),
            &stranger_device,
        )
        .await;
    assert_eq!(joined_with_old_name["name"], "Widgets");
    assert_eq!(joined_with_old_name["created"], false);

    // Both are members; saving into the project uses the request context.
    let context =
        json!({"project_id": id, "repository": "github.com/acme/widgets", "branch": "feature-x"});
    let rule = app
        .post(
            "remember",
            json!({"kind": "rule", "title": "Use cents", "summary": "Money is integer cents.", "reason": "float drift", "context": context}),
            &author_device,
        )
        .await;
    let rule_id = rule["id"].as_str().unwrap().to_owned();
    assert_eq!(rule["deduplicated"], false);
    let personal = app
        .post(
            "remember",
            json!({"kind": "note", "title": "My scratch", "summary": "private is a summary of the test", "scope": "personal"}),
            &author_device,
        )
        .await;
    let again = app
        .post(
            "remember",
            json!({"kind": "rule", "title": "Use cents", "summary": "Money is integer cents.", "reason": "float drift", "context": context}),
            &author_device,
        )
        .await;
    assert_eq!(
        (again["id"].as_str(), again["deduplicated"].as_bool()),
        (Some(rule_id.as_str()), Some(true))
    );

    let seen = app
        .get(&format!("projects/{id}/memories"), &stranger_device)
        .await;
    let seen_ids: Vec<&str> = seen
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(seen_ids, vec![rule_id.as_str()]);
    let (status, _) = app
        .send(
            "GET",
            &format!("memories/{}", personal["id"].as_str().unwrap()),
            json!(null),
            Some(&stranger_device),
        )
        .await;
    assert_eq!(status, 404, "another person's personal memory is not found");
    let all = app.get("memories", &stranger).await;
    assert_eq!(all.as_array().unwrap().len(), 1);

    // Branches are derived from the memories; a non-member sees no project.
    let branches = app
        .get(&format!("projects/{id}/branches"), &author_device)
        .await;
    assert_eq!(branches[0]["branch"], "feature-x");
    let outsider = app.member(&admin, "outsider").await;
    let (status, _) = app
        .send(
            "GET",
            &format!("projects/{id}/memories"),
            json!(null),
            Some(&outsider),
        )
        .await;
    assert_eq!(status, 404);
    assert_eq!(app.get("projects", &outsider).await, json!([]));

    // Conflicts come from `contradicts` links.
    let other = app
        .post("remember", json!({"kind": "rule", "title": "Use floats", "summary": "Money is a float. is a summary of the test", "reason": "simple and the test says why", "context": context, "contradicts": rule_id}), &stranger_device)
        .await;
    let conflicts = app
        .get(&format!("projects/{id}/conflicts"), &author_device)
        .await;
    assert_eq!(conflicts[0]["memory_id"], other["id"]);
    assert_eq!(conflicts[0]["contradicts_id"], rule_id);

    // Ownership: only the owner (or an admin) transfers, and only to a member.
    let members = app.get(&format!("projects/{id}/members"), &author).await;
    let stranger_person = members
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "member")
        .unwrap()["person_id"]
        .clone();
    let (status, _) = app
        .send(
            "POST",
            &format!("projects/{id}/ownership"),
            json!({"personId": stranger_person}),
            Some(&stranger),
        )
        .await;
    assert_eq!(status, 403);
    app.ok(
        "POST",
        &format!("projects/{id}/ownership"),
        json!({"personId": stranger_person}),
        Some(&author),
    )
    .await;
    let listed = app.get("projects", &stranger).await;
    assert_eq!(listed[0]["role"], "owner");
    // Rename is a console action: a device token is refused, even the owner's.
    let (status, body) = app
        .send(
            "POST",
            &format!("projects/{id}/rename"),
            json!({"name": "Widgets 2"}),
            Some(&stranger_device),
        )
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (401, Some("session_required"))
    );
    let renamed = app
        .post(
            &format!("projects/{id}/rename"),
            json!({"name": "Widgets 2"}),
            &stranger,
        )
        .await;
    assert_eq!(renamed["name"], "Widgets 2");
    let (status, _) = app
        .send(
            "POST",
            &format!("projects/{id}/rename"),
            json!({"name": "no"}),
            Some(&author),
        )
        .await;
    assert_eq!(status, 403);

    // The author can forget their own memory; it then vanishes for the member too.
    let (status, _) = app
        .send(
            "POST",
            "forget",
            json!({"id": rule_id}),
            Some(&stranger_device),
        )
        .await;
    assert_eq!(status, 404, "only the owner of a memory forgets it");
    let (status, _) = app
        .send(
            "POST",
            "forget",
            json!({"id": rule_id}),
            Some(&author_device),
        )
        .await;
    assert_eq!(status, 200);
    let seen = app
        .get(&format!("projects/{id}/memories"), &stranger_device)
        .await;
    assert!(!ids(&json!({"results": seen})).contains(&rule_id));
    let (status, body) = app
        .send("POST", "remember", json!({"kind": "rule", "title": "Use cents", "summary": "Money is integer cents.", "reason": "float drift", "context": context}), Some(&author_device))
        .await;
    assert_eq!((status, body["code"].as_str()), (409, Some("conflict")));
    let _ = PASSWORD;
    app.finish().await;
}

#[tokio::test]
async fn local_repository_names_do_not_identify_projects() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "laptop").await;
    let first = app
        .post(
            "projects/init",
            json!({"repository": "local:repo"}),
            &device,
        )
        .await;
    let second = app
        .post(
            "projects/init",
            json!({"repository": "local:repo"}),
            &device,
        )
        .await;
    assert_eq!(first["created"], true);
    assert_eq!(second["created"], true);
    assert_ne!(first["project_id"], second["project_id"]);

    let named = app
        .post(
            "projects/init",
            json!({"repository": "local:repo", "name": "Other"}),
            &device,
        )
        .await;
    assert_eq!(named["created"], true);
    assert_eq!(named["name"], "Other");
    assert_ne!(named["project_id"], first["project_id"]);
    assert_ne!(named["project_id"], second["project_id"]);

    let again = app
        .post(
            "projects/init",
            json!({"repository": "local:repo", "project_id": first["project_id"]}),
            &device,
        )
        .await;
    assert_eq!(again["created"], false);
    assert_eq!(again["project_id"], first["project_id"]);
    app.finish().await;
}

#[tokio::test]
async fn init_registers_a_committed_project_the_server_lost() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let owner = app.device(&admin, "owner-laptop").await;
    let id = "proj_zzzzzzzzzzzzzzzzzzzzzzzzzz";
    // An unknown id (a wiped server) is not registered on a plain request: whoever asks first
    // would own it, so the caller has to confirm.
    let (status, refused) = app
        .send(
            "POST",
            "projects/init",
            json!({"project_id": id, "repository": "github.com/acme/widgets", "name": "Widgets"}),
            Some(&owner),
        )
        .await;
    assert_eq!(status, 409, "{refused}");
    assert!(
        refused.to_string().contains("cairn init --yes"),
        "{refused}"
    );
    assert_eq!(app.get("projects", &owner).await, json!([]));
    // Confirmed, it is created again with the same id, owned by the caller.
    let restored = app
        .post(
            "projects/init",
            json!({"project_id": id, "repository": "github.com/acme/widgets", "name": "Widgets",
                   "confirm_register": true}),
            &owner,
        )
        .await;
    assert_eq!(restored["created"], true);
    assert_eq!(restored["project_id"], id);
    assert_eq!(restored["repository"], "github.com/acme/widgets");
    let listed = app.get("projects", &owner).await;
    assert_eq!(listed[0]["project_id"], id, "{listed}");
    // Asking again joins the project that now exists.
    let again = app
        .post(
            "projects/init",
            json!({"project_id": id, "repository": "github.com/acme/widgets", "name": "Widgets"}),
            &owner,
        )
        .await;
    assert_eq!(
        (again["created"].clone(), again["project_id"].clone()),
        (json!(false), json!(id))
    );
    // An id that is not shaped like ours is refused, not created.
    let (status, _) = app
        .send(
            "POST",
            "projects/init",
            json!({"project_id": "x", "repository": "github.com/acme/widgets"}),
            Some(&owner),
        )
        .await;
    assert_eq!(status, 400);
    app.finish().await;
}

#[tokio::test]
async fn bad_save_input_is_a_400_with_a_message() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "d").await;
    for (body, needle) in [
        (
            json!({"kind": "rule", "title": "A long enough title", "summary": "A summary that says what is true.", "reason": "a stated reason", "colour": "red"}),
            "colour",
        ),
        (
            json!({"kind": "rule", "title": "A long enough title", "summary": "A summary that says what is true.", "reason": "a stated reason", "fields": {"symptom": "x"}}),
            "symptom",
        ),
        (
            json!({"kind": "handoff", "title": "A long enough title", "summary": "A summary that says what is true.", "reason": "a stated reason"}),
            "handoff",
        ),
        (
            json!({"kind": "rule", "title": "A long enough title", "summary": "A summary that says what is true."}),
            "reason",
        ),
        (
            json!({"kind": "rule", "title": "A long enough title", "summary": "A summary that says what is true.", "reason": "a stated reason", "scope": "project"}),
            "project",
        ),
        (
            json!({"kind": "rule", "title": " ", "summary": "A summary that says what is true.", "reason": "a stated reason"}),
            "title must not be empty",
        ),
    ] {
        let (status, reply) = app.send("POST", "remember", body, Some(&device)).await;
        assert_eq!(status, 400, "{reply}");
        assert!(reply["error"].as_str().unwrap().contains(needle), "{reply}");
    }
    app.finish().await;
}

#[tokio::test]
async fn removed_routes_are_gone() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    for (method, path) in [
        ("POST", "/cairn-memory/correct"),
        ("POST", "/cairn-memory/import"),
        ("POST", "/cairn-memory/projects"),
        ("POST", "/cairn-memory/projects/verify"),
    ] {
        let response = tower::ServiceExt::oneshot(
            app.router.clone(),
            axum::http::Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {admin}"))
                .header("content-type", "application/json")
                .body(axum::body::Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
        assert!(
            matches!(response.status().as_u16(), 404 | 405),
            "{method} {path}: {}",
            response.status()
        );
    }
    app.finish().await;
}

#[tokio::test]
async fn account_errors_use_the_texts_the_console_expects() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let user = app.member(&admin, "user").await;
    let (status, body) = app
        .send(
            "POST",
            "auth/login",
            json!({"email": "user@example.test", "password": "wrong password!"}),
            None,
        )
        .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (401, Some("invalid account credentials"))
    );
    let (status, body) = app
        .send("POST", "auth/password", json!({"current_password": "not the password", "new_password": "changed horse battery staple"}), Some(&user))
        .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (401, Some("invalid account credentials"))
    );
    let (status, body) = app.send("GET", "projects", json!(null), None).await;
    assert_eq!(
        (status, body["error"].as_str(), body["code"].as_str()),
        (
            401,
            Some("Bearer token is required"),
            Some("session_required")
        )
    );
    let (status, _) = app
        .send(
            "POST",
            "auth/register",
            json!({"email": "x@example.test", "password": PASSWORD}),
            Some(&user),
        )
        .await;
    assert_eq!(status, 403, "only the admin creates accounts");
    // A password reset by the admin signs the user out; logout revokes the session.
    let person = app.get("auth/me", &user).await["person_id"]
        .as_str()
        .unwrap()
        .to_owned();
    app.ok(
        "POST",
        &format!("auth/accounts/{person}/password"),
        json!({"password": "reset horse battery staple"}),
        Some(&admin),
    )
    .await;
    let (status, body) = app.send("GET", "projects", json!(null), Some(&user)).await;
    assert_eq!(
        (status, body["error"].as_str(), body["code"].as_str()),
        (
            401,
            Some("account session is expired or revoked"),
            Some("session_required")
        )
    );
    app.finish().await;
}

#[tokio::test]
async fn missing_gotcha_fields_return_one_line_of_coaching() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let device = app.device(&admin, "d").await;
    let thin = json!({"kind": "gotcha", "title": "Requests stall on pool exhaustion", "summary": "Requests hang when the pool is exhausted.", "reason": "Observed in production when all connections were busy."});
    let (status, reply) = app.send("POST", "remember", thin, Some(&device)).await;
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["status"], "active");
    assert_eq!(
        reply["hints"],
        json!(["Fill in the gotcha's symptom, cause and fix."])
    );
    app.finish().await;
}

#[tokio::test]
async fn repository_fallback_status_is_attached_by_repository() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let owner = app.device(&admin, "owner").await;
    let member_web = app.member(&admin, "member").await;
    let member = app.device(&member_web, "member-laptop").await;
    let stranger_web = app.member(&admin, "stranger").await;
    let stranger = app.device(&stranger_web, "stranger-laptop").await;
    let repo = "github.com/acme/widgets";
    let id = app.project(&owner, repo).await;
    app.post(
        "projects/init",
        json!({"project_id": id, "repository": repo, "name": "widgets"}),
        &member,
    )
    .await;

    let attached = app.post(
        "observe",
        json!({"event_id": "e1", "session_id": "s1", "kind": "prompt", "content": "work in the repo",
               "context": {"repository": repo, "branch": "main"}}),
        &member,
    )
    .await;
    assert_eq!(
        attached["project_attachment"]["status"],
        "attached_by_repository"
    );
    let brief = app
        .post(
            "session/brief",
            json!({"session_id": "b1", "context": {"repository": repo}}),
            &member,
        )
        .await;
    assert_eq!(brief["project_id"], id);

    let other = app.member(&admin, "other").await;
    let other_device = app.device(&other, "other-laptop").await;
    let fork = app.project(&other_device, repo).await;
    app.post(
        "projects/init",
        json!({"project_id": fork, "repository": repo, "name": "widgets"}),
        &member,
    )
    .await;
    let ambiguous = app
        .post(
            "observe",
            json!({"event_id": "e2", "session_id": "s2", "kind": "prompt", "content": "which fork",
               "context": {"repository": repo}}),
            &member,
        )
        .await;
    assert_eq!(ambiguous["project_attachment"]["status"], "personal");

    let leaked = app
        .post(
            "observe",
            json!({"event_id": "e3", "session_id": "s3", "kind": "prompt", "content": "not mine",
               "context": {"repository": repo}}),
            &stranger,
        )
        .await;
    assert_eq!(leaked["project_attachment"]["status"], "personal");
    assert!(leaked.get("project_id").is_none());
    let quiet = app
        .post(
            "session/brief",
            json!({"session_id": "b2", "context": {"repository": repo}}),
            &stranger,
        )
        .await;
    assert!(quiet.get("project_id").is_none());
    app.finish().await;
}

#[tokio::test]
async fn project_sessions_member_only() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let author = app.member(&admin, "author").await;
    let stranger = app.member(&admin, "stranger").await;
    let author_device = app.device(&author, "author-laptop").await;
    let stranger_device = app.device(&stranger, "stranger-laptop").await;

    let project = app
        .post(
            "projects/init",
            json!({"repository": "github.com/acme/sessions-test", "name": "SessionsTest"}),
            &author_device,
        )
        .await;
    let pid = project["project_id"].as_str().unwrap();

    // 3. Non-member GET .../sessions returns 404
    let (status, body) = app
        .send(
            "GET",
            &format!("projects/{pid}/sessions?days=30"),
            json!(null),
            Some(&stranger_device),
        )
        .await;
    assert_eq!(status, 404, "{body}");

    // Create a session for author:
    let sess = "sess-1";
    app.post(
        "observe",
        json!({
            "event_id": "ev-1",
            "session_id": sess,
            "kind": "prompt",
            "content": "working on something",
            "context": {"project_id": pid, "repository": "github.com/acme/sessions-test", "branch": "main"}
        }),
        &author_device,
    )
    .await;

    // 4. An agent memory_save during a session is counted in agent_saves:
    let save_res = app
        .post(
            "remember",
            json!({
                "kind": "fact",
                "title": "Agent save test",
                "summary": "Agent saved this during session",
                "reason": "Test agent saves during session",
                "context": {"project_id": pid, "repository": "github.com/acme/sessions-test", "branch": "main"}
            }),
            &author_device,
        )
        .await;
    assert!(save_res.get("id").is_some());

    app.post("session/end", json!({"session_id": sess}), &author_device)
        .await;

    // Member GET .../sessions returns rows newest first, <= 50
    let (status, body) = app
        .send(
            "GET",
            &format!("projects/{pid}/sessions?days=30"),
            json!(null),
            Some(&author_device),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let rows = body.as_array().expect("array of sessions");
    assert!(!rows.is_empty());
    assert!(rows.len() <= 50);
    let s = &rows[0];
    assert_eq!(s["session_id"], sess);
    assert_eq!(s["agent_saves"], 1);

    sqlx::query(
        "INSERT INTO sessions (person_id, session_id, project_id, started_at)
         SELECT person_id, 'older-' || n, project_id, now() - make_interval(secs => n)
           FROM sessions CROSS JOIN generate_series(1, 55) AS n WHERE session_id = $1",
    )
    .bind(sess)
    .execute(app.db.store.pool())
    .await
    .unwrap();
    let rows = app
        .get(&format!("projects/{pid}/sessions?days=7"), &author_device)
        .await;
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 50);
    assert!(
        rows.windows(2)
            .all(|pair| pair[0]["started_at"].as_i64() >= pair[1]["started_at"].as_i64())
    );
    let (invalid, _) = app
        .send(
            "GET",
            &format!("projects/{pid}/sessions?days=8"),
            json!(null),
            Some(&author_device),
        )
        .await;
    assert_eq!(invalid, 400);

    app.finish().await;
}

#[tokio::test]
async fn device_fixes_scoped() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let a = app.device(&admin, "a").await;
    let b = app.device(&admin, "b").await;
    let (device, _) = app.db.store.authenticate_device(&a).await.unwrap();
    sqlx::query("INSERT INTO fixes (fix_id,detector,fingerprint,device_id,fix_type,status,first_seen_at,last_seen_at,action) VALUES ('fx_scoped','F5','fp',$1,'computer','queued',now(),now(),$2)")
        .bind(&device.device_id).bind(json!({"type":"run_update","target":"0.0.1"})).execute(app.db.store.pool()).await.unwrap();
    let (status, fixes) = app.send("GET", "device/fixes", json!(null), Some(&a)).await;
    assert_eq!(status, 200, "{fixes}");
    assert_eq!(fixes.as_array().unwrap().len(), 1);
    let (status, fixes) = app.send("GET", "device/fixes", json!(null), Some(&b)).await;
    assert_eq!((status, fixes), (200, json!([])));
    let (status, _) = app
        .send(
            "POST",
            "device/fixes/fx_scoped",
            json!({"result":"applied"}),
            Some(&b),
        )
        .await;
    assert_eq!(status, 404);
    let state: String = sqlx::query_scalar("SELECT status FROM fixes WHERE fix_id='fx_scoped'")
        .fetch_one(app.db.store.pool())
        .await
        .unwrap();
    assert_eq!(state, "queued");
    for (id, result, reason, expected, audit) in [
        ("fx_scoped", "applied", None, "done", "fix_done"),
        (
            "fx_current",
            "skipped",
            Some("already_current"),
            "resolved",
            "fix_done",
        ),
        (
            "fx_failed",
            "skipped",
            Some("file_exists"),
            "failed",
            "fix_failed",
        ),
    ] {
        if id != "fx_scoped" {
            sqlx::query("INSERT INTO fixes (fix_id,detector,fingerprint,device_id,fix_type,status,first_seen_at,last_seen_at,action) VALUES ($1,'F5',$1,$2,'computer','queued',now(),now(),'{}')")
                .bind(id).bind(&device.device_id).execute(app.db.store.pool()).await.unwrap();
        }
        let body = if let Some(reason) = reason {
            json!({"result":result,"reason":reason})
        } else {
            json!({"result":result})
        };
        let (status, _) = app
            .send("POST", &format!("device/fixes/{id}"), body, Some(&a))
            .await;
        assert_eq!(status, 200);
        let (state, error): (String, Option<String>) =
            sqlx::query_as("SELECT status,error FROM fixes WHERE fix_id=$1")
                .bind(id)
                .fetch_one(app.db.store.pool())
                .await
                .unwrap();
        assert_eq!(state, expected);
        assert_eq!(
            error.as_deref(),
            if expected == "failed" { reason } else { None }
        );
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_events WHERE action=$1 AND subject_id=$2 AND metadata=$3",
        )
        .bind(audit)
        .bind(id)
        .bind(json!({"fix_id":id}))
        .fetch_one(app.db.store.pool())
        .await
        .unwrap();
        assert_eq!(count, 1);
    }
    app.finish().await;
}
#[tokio::test]
async fn personal_observation_in_a_project_session_is_distilled_personally() {
    let Some(app) = App::new().await else { return };
    let web = app.admin("mixed-capture").await;
    let device = app.device(&web, "agent").await;
    let project = app.project(&device, "example.test/mixed-capture").await;
    for (event, context, expected) in [
        ("project", json!({"project_id": project}), "attached"),
        ("personal", json!({}), "personal"),
    ] {
        let reply = app.post("observe", json!({
            "event_id":event,"session_id":"mixed","kind":"prompt","context":context,
            "content":format!("Decision: keep the {event} note private\nReason: mixed capture contexts")
        }), &device).await;
        assert_eq!(reply["project_attachment"]["status"], expected);
    }
    app.post("session/end", json!({"session_id":"mixed"}), &device)
        .await;
    app.settle().await;
    let scopes: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT scope, project_id FROM memories WHERE kind = 'decision'")
            .fetch_all(app.db.store.pool())
            .await
            .unwrap();
    assert_eq!(scopes, vec![("personal".to_owned(), None); 2]);
    app.finish().await;
}

#[tokio::test]
async fn project_archive_keeps_console_reads_and_hides_agent_memory_until_restore() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("archive-admin").await;
    let owner = app.member(&admin, "archive-owner").await;
    let member = app.member(&admin, "archive-member").await;
    let device = app.device(&owner, "archive-laptop").await;
    let repository = "github.com/acme/e2e-archive";
    let project = app
        .post(
            "projects/init",
            json!({"repository": repository, "name": "E2E Archive"}),
            &device,
        )
        .await;
    let pid = project["project_id"].as_str().unwrap();
    let init = json!({"project_id": pid, "repository": repository});
    app.post("projects/init", init.clone(), &member).await;
    let archive = format!("projects/{pid}/archive");
    let restore = format!("projects/{pid}/restore");
    let context = json!({"project_id": pid, "repository": repository, "branch": "main", "files": ["src/archive.rs"]});
    let note = json!({"kind": "rule", "scope": "project", "title": "Preserve blueprints for restoration",
        "summary": "Keep the blueprints for the archive.", "reason": "They allow exact restoration.",
        "context": context, "detail": "Detailed blueprint."});
    let memory = app.save(&device, note.clone()).await;
    let get_body = json!({"ids": [memory], "detail": true});
    let search_body = json!({"query": "preserve blueprints", "context": context});
    let brief_body = json!({"session_id": "e2e-archive", "context": context});
    let topic_body = json!({"path": "src/archive.rs", "context": context});
    assert!(ids(&app.post("search", search_body.clone(), &device).await).contains(&memory));
    let before = app.post("get", get_body.clone(), &device).await;
    assert_eq!(before["memories"][0]["detail"], "Detailed blueprint.");
    assert_eq!(before["memories"][0]["trust"], "normal · used 1×");
    assert!(
        app.post("session/brief", brief_body.clone(), &device).await["brief"]
            .as_str()
            .unwrap()
            .contains("Preserve blueprints")
    );
    assert_eq!(
        app.post("session/topic", topic_body.clone(), &device).await["notes"][0]["id"],
        memory
    );

    assert_eq!(
        app.send("POST", &archive, json!(null), Some(&member))
            .await
            .0,
        403
    );
    assert_eq!(
        app.send("POST", &archive, json!(null), Some(&device))
            .await
            .0,
        401
    );
    let archived = app.post(&archive, json!(null), &owner).await;
    assert!(archived["archived_at"].is_number());
    assert_eq!(app.post(&archive, json!(null), &owner).await, archived);
    assert_eq!(
        app.send("POST", &restore, json!(null), Some(&member))
            .await
            .0,
        403
    );

    assert!(ids(&app.post("search", search_body.clone(), &device).await).is_empty());
    assert_eq!(
        app.post("get", get_body.clone(), &device).await,
        json!({"memories": [], "missing": [memory]})
    );
    let brief = app.post("session/brief", brief_body.clone(), &device).await;
    assert!(
        !brief["brief"]
            .as_str()
            .unwrap()
            .contains("Preserve blueprints")
    );
    assert!(brief.get("check").is_none());
    assert_eq!(
        app.post("session/topic", topic_body, &device).await["notes"],
        json!([])
    );
    let graph = app.get(&format!("graph?projectId={pid}"), &device).await;
    assert_eq!(graph["nodes"], json!([]));
    assert_eq!(graph["edges"], json!([]));

    let detail = format!("memories/{memory}");
    let scoped = format!("memories?projectId={pid}");
    assert_eq!(
        app.send("GET", &detail, json!(null), Some(&device)).await.0,
        404
    );
    assert_eq!(app.get(&scoped, &device).await, json!([]));
    assert_eq!(app.get("memories", &member).await, json!([]));
    assert_eq!(app.get(&scoped, &member).await[0]["id"], memory);
    let view = app.get(&detail, &member).await;
    assert_eq!(view["project_archived"], true);
    assert_eq!(view["detail"], "Detailed blueprint.");
    assert_eq!(view["usage"]["shown"], 2);
    assert_eq!(view["usage"]["detail_opened"], 1);
    let (status, _) = app
        .send("POST", "forget", json!({"id": memory}), Some(&owner))
        .await;
    assert_eq!(status, 409);

    let message = "project E2E Archive is archived; restore it in the web console";
    for (path, body) in [
        ("remember", note.clone()),
        ("projects/init", init),
        ("projects/init", json!({"repository": repository})),
    ] {
        let (status, body) = app.send("POST", path, body, Some(&device)).await;
        assert_eq!(status, 409, "{body}");
        assert_eq!(body["error"], message);
    }
    for scope in ["personal", "global"] {
        let mut allowed = note.clone();
        allowed["scope"] = json!(scope);
        app.save(&device, allowed).await;
    }

    // Admin can restore even without project membership.
    assert!(app.post(&restore, json!(null), &admin).await["archived_at"].is_null());
    let mut expected = before;
    expected["memories"][0]["trust"] = json!("normal · used 2×");
    assert_eq!(app.post("get", get_body, &device).await, expected);
    assert!(ids(&app.post("search", search_body, &device).await).contains(&memory));
    assert!(
        app.post(
            "session/brief",
            json!({"session_id": "e2e-archive-restored", "context": context}),
            &device
        )
        .await["brief"]
            .as_str()
            .unwrap()
            .contains("Preserve blueprints")
    );
    let events: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_events WHERE subject_id = $1 AND action IN ('project_archived', 'project_restored') ORDER BY created_at"
    ).bind(pid).fetch_all(app.db.store.pool()).await.unwrap();
    assert_eq!(events, ["project_archived", "project_restored"]);
    app.finish().await;
}
