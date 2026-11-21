//! Shared helpers: an in-process router on a `TestDb` schema, and JSON requests against it.
#![allow(dead_code)]

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use cairn_server::{AppState, LocalEmbeddings, router, worker};
use cairn_store::testing::TestDb;
use serde_json::{Value, json};
use tower::ServiceExt;

pub const PASSWORD: &str = "correct horse battery staple";

pub struct App {
    pub db: TestDb,
    pub state: AppState,
    pub router: Router,
}

impl App {
    pub async fn new() -> Option<Self> {
        Self::with_model(None).await
    }

    pub async fn with_model(model: Option<LocalEmbeddings>) -> Option<Self> {
        let db = TestDb::new().await?;
        let state = AppState::new(db.store.clone(), model);
        let router = router(state.clone());
        Some(Self { db, state, router })
    }

    /// A server started with `CAIRN_PUBLIC_URL` set.
    pub async fn with_public_url(url: &str) -> Option<Self> {
        let db = TestDb::new().await?;
        let state = AppState::new(db.store.clone(), None).with_public_url(Some(url.to_owned()));
        let router = router(state.clone());
        Some(Self { db, state, router })
    }

    pub async fn send(
        &self,
        method: &str,
        path: &str,
        body: Value,
        token: Option<&str>,
    ) -> (u16, Value) {
        self.send_with(method, path, body, token, &[]).await
    }

    /// `send` with extra request headers (a `user-agent`, a `host`).
    pub async fn send_with(
        &self,
        method: &str,
        path: &str,
        body: Value,
        token: Option<&str>,
        headers: &[(&str, &str)],
    ) -> (u16, Value) {
        let (status, _, bytes) = self.send_raw(method, path, body, token, headers).await;
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, value)
    }

    /// Status, response headers and the raw body.
    pub async fn send_raw(
        &self,
        method: &str,
        path: &str,
        body: Value,
        token: Option<&str>,
        headers: &[(&str, &str)],
    ) -> (u16, axum::http::HeaderMap, axum::body::Bytes) {
        let mut builder = Request::builder()
            .method(method)
            .uri(format!("/cairn-memory/{path}"))
            .header("content-type", "application/json");
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let response = self
            .router
            .clone()
            .oneshot(builder.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, headers, bytes)
    }

    /// A request that must succeed.
    pub async fn ok(&self, method: &str, path: &str, body: Value, token: Option<&str>) -> Value {
        let (status, body) = self.send(method, path, body, token).await;
        assert!(
            (200..300).contains(&status),
            "{method} {path}: {status} {body}"
        );
        body
    }

    pub async fn post(&self, path: &str, body: Value, token: &str) -> Value {
        self.ok("POST", path, body, Some(token)).await
    }

    pub async fn get(&self, path: &str, token: &str) -> Value {
        self.ok("GET", path, Value::Null, Some(token)).await
    }

    /// Saves a memory as an agent; returns its id. `body` is the `remember` body.
    pub async fn save(&self, token: &str, body: Value) -> String {
        self.post("remember", body, token).await["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// A project the device's person owns; returns its id.
    pub async fn project(&self, device: &str, repository: &str) -> String {
        self.post("projects/init", json!({"repository": repository}), device)
            .await["project_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// The bootstrapped admin's web session token.
    pub async fn admin(&self, name: &str) -> String {
        let reply = self
            .ok(
                "POST",
                "auth/bootstrap",
                json!({"email": format!("{name}@example.test"), "password": PASSWORD}),
                None,
            )
            .await;
        reply["token"].as_str().unwrap().to_owned()
    }

    /// Another account's web session token (registered by the admin, then logged in).
    pub async fn member(&self, admin_token: &str, name: &str) -> String {
        let email = format!("{name}@example.test");
        self.post(
            "auth/register",
            json!({"email": email, "password": PASSWORD}),
            admin_token,
        )
        .await;
        let reply = self
            .ok(
                "POST",
                "auth/login",
                json!({"email": email, "password": PASSWORD}),
                None,
            )
            .await;
        reply["token"].as_str().unwrap().to_owned()
    }

    /// A device token for the account behind `web_token`.
    pub async fn device(&self, web_token: &str, name: &str) -> String {
        let reply = self
            .post("devices/register", json!({"name": name}), web_token)
            .await;
        reply["token"].as_str().unwrap().to_owned()
    }

    /// Runs the background jobs until none is due (distill, embed, consolidate in turn).
    pub async fn settle(&self) {
        for _ in 0..4 {
            worker::tick(&self.state).await;
        }
    }

    pub async fn finish(self) {
        drop(self.router);
        self.db.finish().await;
    }
}

/// Ids of the results of a search-like reply.
pub fn ids(reply: &Value) -> Vec<String> {
    reply["results"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_owned())
        .collect()
}
