//! Step 7a auth hardening: stable error codes, equal login work on every path, no lockout.

mod common;
use common::{App, PASSWORD};
use serde_json::json;
use std::time::Instant;

fn median(mut times: Vec<f64>) -> f64 {
    times.sort_by(f64::total_cmp);
    times[times.len() / 2]
}

#[tokio::test]
async fn login_unknown_email_and_wrong_password_same_code() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    app.member(&admin, "user").await;
    for email in ["user@example.test", "nobody@example.test", "not-an-email"] {
        let (status, body) = app
            .send(
                "POST",
                "auth/login",
                json!({"email": email, "password": "wrong password here"}),
                None,
            )
            .await;
        assert_eq!(status, 401, "{email}: {body}");
        assert_eq!(body["code"], "invalid_credentials", "{email}");
        assert_eq!(body["error"], "invalid account credentials", "{email}");
    }
    // A wrong current password is the same code, so the web never mistakes it for a lost session.
    let user = app
        .ok(
            "POST",
            "auth/login",
            json!({"email": "user@example.test", "password": PASSWORD}),
            None,
        )
        .await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, body) = app
        .send(
            "POST",
            "auth/password",
            json!({"current_password": "not the password", "new_password": "changed horse battery staple"}),
            Some(&user),
        )
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (401, Some("invalid_credentials"))
    );
    app.finish().await;
}

#[tokio::test]
async fn login_runs_verify_on_every_path() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    app.member(&admin, "user").await;
    let mut unknown = Vec::new();
    let mut wrong = Vec::new();
    for i in 0..5 {
        let started = Instant::now();
        app.send(
            "POST",
            "auth/login",
            json!({"email": format!("nobody{i}@example.test"), "password": "wrong password here"}),
            None,
        )
        .await;
        unknown.push(started.elapsed().as_secs_f64());
        let started = Instant::now();
        app.send(
            "POST",
            "auth/login",
            json!({"email": "user@example.test", "password": "wrong password here"}),
            None,
        )
        .await;
        wrong.push(started.elapsed().as_secs_f64());
    }
    let (unknown, wrong) = (median(unknown), median(wrong));
    assert!(
        unknown >= 0.5 * wrong,
        "unknown email answered in {unknown:.4}s, wrong password in {wrong:.4}s"
    );
    app.finish().await;
}

#[tokio::test]
async fn no_lockout_after_many_failures() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    app.member(&admin, "user").await;
    for _ in 0..50 {
        let (status, body) = app
            .send(
                "POST",
                "auth/login",
                json!({"email": "user@example.test", "password": "wrong password here"}),
                None,
            )
            .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (401, Some("invalid_credentials"))
        );
    }
    let (status, body) = app
        .send(
            "POST",
            "auth/login",
            json!({"email": "user@example.test", "password": PASSWORD}),
            None,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    // The admin can register many accounts in a row: no per-email counter exists anywhere.
    for _ in 0..12 {
        let (status, _) = app
            .send(
                "POST",
                "auth/register",
                json!({"email": "user@example.test", "password": PASSWORD}),
                Some(&admin),
            )
            .await;
        assert_eq!(
            status, 400,
            "a duplicate is a validation error, never a 429"
        );
    }
    app.finish().await;
}

#[tokio::test]
async fn expired_session_is_session_required() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let user = app.member(&admin, "user").await;
    let device = app.device(&user, "laptop").await;
    // Missing bearer, unknown bearer and a device token on a web route.
    for token in [None, Some("nope"), Some(device.as_str())] {
        for path in ["stats", "auth/audit", "devices"] {
            let (status, body) = app.send("GET", path, json!(null), token).await;
            assert_eq!(status, 401, "{path} {token:?}: {body}");
            assert_eq!(body["code"], "session_required", "{path} {token:?}");
        }
    }
    // Routes that take a device token too still send a missing or unknown bearer to sign-in.
    for token in [None, Some("nope")] {
        let (status, body) = app.send("GET", "projects", json!(null), token).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (401, Some("session_required"))
        );
    }
    // Expired, then revoked sessions.
    sqlx::query("UPDATE web_sessions SET expires_at = now() - interval '1 second'")
        .execute(app.db.store.pool())
        .await
        .unwrap();
    for path in ["stats", "projects", "auth/me", "auth/accounts"] {
        let (status, body) = app.send("GET", path, json!(null), Some(&user)).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (401, Some("session_required")),
            "{path}: {body}"
        );
    }
    let again = app
        .ok(
            "POST",
            "auth/login",
            json!({"email": "user@example.test", "password": PASSWORD}),
            None,
        )
        .await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    app.post("auth/logout", json!({}), &again).await;
    let (status, body) = app.send("GET", "stats", json!(null), Some(&again)).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (401, Some("session_required"))
    );
    app.finish().await;
}

#[tokio::test]
async fn device_failures_keep_the_unauthorized_code() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    let user = app.member(&admin, "user").await;
    let device = app.device(&user, "laptop").await;
    let id = app.get("devices", &user).await[0]["device_id"]
        .as_str()
        .unwrap()
        .to_owned();
    app.post("devices/revoke", json!({"deviceId": id}), &user)
        .await;
    let (status, body) = app
        .send("GET", "projects", json!(null), Some(&device))
        .await;
    assert_eq!((status, body["code"].as_str()), (401, Some("unauthorized")));
    let (status, body) = app.send("POST", "observe", json!({}), Some(&device)).await;
    assert_eq!((status, body["code"].as_str()), (401, Some("unauthorized")));
    app.finish().await;
}

#[tokio::test]
async fn login_expires_at_is_number() {
    let Some(app) = App::new().await else { return };
    let boot = app
        .ok(
            "POST",
            "auth/bootstrap",
            json!({"email": "admin@example.test", "password": PASSWORD}),
            None,
        )
        .await;
    assert!(boot["expires_at"].is_i64(), "{boot}");
    let login = app
        .ok(
            "POST",
            "auth/login",
            json!({"email": "admin@example.test", "password": PASSWORD}),
            None,
        )
        .await;
    let expires = login["expires_at"].as_i64().expect("a number");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(expires > now + 29 * 86_400 && expires <= now + 31 * 86_400);
    app.finish().await;
}

#[tokio::test]
async fn admin_job_routes_are_read_only() {
    let Some(app) = App::new().await else { return };
    let admin = app.admin("admin").await;
    app.get("admin/jobs", &admin).await;
    for path in ["admin/jobs/job_x/requeue", "admin/jobs/job_x/discard"] {
        let (status, _) = app.send("POST", path, json!({}), Some(&admin)).await;
        assert!(matches!(status, 404 | 405), "{path}: {status}");
    }
    app.finish().await;
}
