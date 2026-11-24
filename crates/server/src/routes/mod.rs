//! Routes by area. Each file owns its request types and handlers; this file only lists paths.

mod account;
mod admin;
mod capture;
mod device;
mod fixes;
mod health;
mod instructions;
mod memory;
mod project;
mod retrieve;
mod stats;

use crate::AppState;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};

/// An import file of 5,000 memories is a few MB; axum's default limit is 2 MB.
const IMPORT_BODY_LIMIT: usize = 16 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/cairn-memory/health", get(health::health))
        .route("/cairn-memory/livez", get(health::livez))
        // account
        .route("/cairn-memory/auth/bootstrap", post(account::bootstrap))
        .route("/cairn-memory/auth/register", post(account::register))
        .route("/cairn-memory/auth/login", post(account::login))
        .route("/cairn-memory/auth/logout", post(account::logout))
        .route("/cairn-memory/auth/me", get(account::me))
        .route("/cairn-memory/auth/audit", get(account::audit))
        .route("/cairn-memory/auth/accounts", get(account::accounts))
        .route(
            "/cairn-memory/auth/accounts/{person_id}/password",
            post(account::reset_password),
        )
        .route(
            "/cairn-memory/auth/password",
            post(account::change_password),
        )
        // device
        .route("/cairn-memory/device/fixes", get(device::fixes))
        .route("/cairn-memory/device/fixes/{id}", post(device::finish_fix))
        .route("/cairn-memory/devices", get(device::list))
        .route("/cairn-memory/devices/register", post(device::register))
        .route("/cairn-memory/devices/revoke", post(device::revoke))
        .route("/cairn-memory/devices/rotate", post(device::rotate))
        // agent instructions
        .route(
            "/cairn-memory/agent-instructions",
            get(instructions::global).post(instructions::set_global),
        )
        .route(
            "/cairn-memory/projects/{project_id}/agent-instructions",
            get(instructions::project).post(instructions::set_project),
        )
        // admin
        .route("/cairn-memory/admin/jobs", get(admin::jobs))
        .route("/cairn-memory/admin/fixes", get(fixes::list))
        .route("/cairn-memory/admin/fixes/count", get(fixes::count))
        .route("/cairn-memory/admin/fixes/{id}", get(fixes::detail))
        .route("/cairn-memory/admin/fixes/{id}/apply", post(fixes::apply))
        .route("/cairn-memory/admin/fixes/{id}/ignore", post(fixes::ignore))
        .route("/cairn-memory/admin/fixes/{id}/copied", post(fixes::copied))
        .route("/cairn-memory/admin/fixes/{id}/done", post(fixes::done))
        .route("/cairn-memory/admin/searches", get(fixes::searches))
        .route(
            "/cairn-memory/admin/memories/{id}/rewrite",
            post(fixes::rewrite),
        )
        .route("/cairn-memory/stats", get(stats::stats))
        .route("/cairn-memory/stats/attention", get(stats::attention))
        // project
        .route("/cairn-memory/projects", get(project::list))
        .route("/cairn-memory/projects/init", post(project::init))
        .route(
            "/cairn-memory/projects/{project_id}/rename",
            post(project::rename),
        )
        .route(
            "/cairn-memory/projects/{project_id}/ownership",
            post(project::transfer),
        )
        .route(
            "/cairn-memory/projects/{project_id}/memories",
            get(project::memories),
        )
        .route(
            "/cairn-memory/projects/{project_id}/members",
            get(project::members),
        )
        .route(
            "/cairn-memory/projects/{project_id}/branches",
            get(project::branches),
        )
        .route(
            "/cairn-memory/projects/{project_id}/sessions",
            get(project::sessions),
        )
        .route(
            "/cairn-memory/projects/{project_id}/conflicts",
            get(project::conflicts),
        )
        // capture
        .route("/cairn-memory/observe", post(capture::observe))
        .route("/cairn-memory/session/lookup", post(capture::lookup))
        .route("/cairn-memory/session/end", post(capture::end))
        .route(
            "/cairn-memory/session/checkpoint",
            post(capture::checkpoint),
        )
        .route(
            "/cairn-memory/session/repo-state",
            post(capture::repo_state),
        )
        // memory
        .route("/cairn-memory/remember", post(memory::remember))
        .route("/cairn-memory/get", post(memory::get_many))
        .route("/cairn-memory/memories", get(memory::list))
        .route("/cairn-memory/memories/export", get(memory::export))
        .route(
            "/cairn-memory/memories/import",
            post(memory::import).layer(DefaultBodyLimit::max(IMPORT_BODY_LIMIT)),
        )
        .route("/cairn-memory/memories/{id}", get(memory::get))
        .route("/cairn-memory/feedback", post(memory::feedback))
        .route("/cairn-memory/forget", post(memory::forget))
        // retrieve
        .route("/cairn-memory/search", post(retrieve::search))
        .route("/cairn-memory/session/brief", post(retrieve::brief))
        .route("/cairn-memory/session/topic", post(retrieve::topic))
        .route("/cairn-memory/graph", get(retrieve::graph))
}
