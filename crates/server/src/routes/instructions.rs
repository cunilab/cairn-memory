//! Agent instructions: the session-start line and the Stop reminder. The global texts are for any
//! signed-in caller to read and a server admin to change; a project's overrides are for its members
//! to read and its owner (or an admin) to change. Web sessions change them, never a device token.

use crate::AppState;
use crate::auth::{Admin, Agent, Web};
use crate::error::ApiResult;
use axum::Json;
use axum::extract::{Path, State};
use cairn_store::instructions::{AgentInstructions, InstructionsUpdate};
use serde::Deserialize;

/// A field left out stays as it is; an empty text clears it (the next level applies).
#[derive(Deserialize)]
pub struct Update {
    #[serde(default)]
    session_instruction: Option<String>,
    #[serde(default)]
    stop_nudge: Option<String>,
    #[serde(default)]
    chat_instruction: Option<String>,
}

impl From<Update> for InstructionsUpdate {
    fn from(update: Update) -> Self {
        Self {
            session_instruction: update.session_instruction,
            stop_nudge: update.stop_nudge,
            chat_instruction: update.chat_instruction,
        }
    }
}

pub async fn global(
    State(state): State<AppState>,
    Agent(account): Agent,
) -> ApiResult<AgentInstructions> {
    Ok(Json(
        state
            .store
            .agent_instructions(&account.person_id, account.is_admin, None)
            .await?,
    ))
}

pub async fn set_global(
    State(state): State<AppState>,
    Admin(account): Admin,
    Json(body): Json<Update>,
) -> ApiResult<AgentInstructions> {
    Ok(Json(
        state
            .store
            .set_agent_instructions(
                &account.person_id,
                account.is_admin,
                None,
                None,
                &body.into(),
            )
            .await?,
    ))
}

pub async fn project(
    State(state): State<AppState>,
    Agent(account): Agent,
    Path(project_id): Path<String>,
) -> ApiResult<AgentInstructions> {
    Ok(Json(
        state
            .store
            .agent_instructions(&account.person_id, account.is_admin, Some(&project_id))
            .await?,
    ))
}

pub async fn set_project(
    State(state): State<AppState>,
    Web(account): Web,
    Path(project_id): Path<String>,
    Json(body): Json<Update>,
) -> ApiResult<AgentInstructions> {
    Ok(Json(
        state
            .store
            .set_agent_instructions(
                &account.person_id,
                account.is_admin,
                None,
                Some(&project_id),
                &body.into(),
            )
            .await?,
    ))
}
