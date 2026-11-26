//! Agent instructions: the session-start line and the Stop reminder, each resolved per field as
//! project override, then global text, then the built-in default. Global texts sit in
//! `server_state`; a project's overrides are columns of `projects`.

use crate::account::audit_tx;
use crate::{Result, Store, StoreError};
use cairn_core::account::ADMIN_ONLY_ERROR;
use cairn_core::instructions::{
    DEFAULT_CHAT_INSTRUCTION, DEFAULT_SESSION_INSTRUCTION, DEFAULT_STOP_NUDGE,
    MAX_INSTRUCTION_BYTES, validate_instruction,
};
use serde::Serialize;
use sqlx::PgConnection;

/// Audit action written for every changed field; metadata has `scope`, `field`, `old` and `new`.
pub const INSTRUCTIONS_AUDIT: &str = "agent_instructions_changed";
const GLOBAL_KEYS: [&str; 3] = [
    "agent_session_instruction",
    "agent_stop_nudge",
    "agent_chat_instruction",
];
const FIELDS: [&str; 3] = ["session_instruction", "stop_nudge", "chat_instruction"];

/// Where the text an agent receives comes from.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InstructionSource {
    Project,
    Global,
    Default,
}

/// One field at one scope.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InstructionField {
    /// What the agent receives for this scope.
    pub text: String,
    pub source: InstructionSource,
    /// The text saved at exactly this scope, `None` when there is none.
    pub own: Option<String>,
    /// What applies when `own` is cleared: the global text or the built-in default.
    pub inherited: String,
    pub inherited_source: InstructionSource,
}

/// The fields at one scope. The chat instruction is global.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AgentInstructions {
    pub session_instruction: InstructionField,
    pub stop_nudge: InstructionField,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub chat_instruction: Option<InstructionField>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub max_bytes: usize,
}

/// A change: `None` leaves a field alone, an empty text clears it.
#[derive(Clone, Debug, Default)]
pub struct InstructionsUpdate {
    pub session_instruction: Option<String>,
    pub stop_nudge: Option<String>,
    pub chat_instruction: Option<String>,
}

fn default_text(field: usize) -> &'static str {
    [
        DEFAULT_SESSION_INSTRUCTION,
        DEFAULT_STOP_NUDGE,
        DEFAULT_CHAT_INSTRUCTION,
    ][field]
}

fn field(
    own: Option<String>,
    global: Option<&str>,
    default: &str,
    project: bool,
) -> InstructionField {
    let (inherited, inherited_source) = match global.filter(|text| !text.is_empty()) {
        Some(text) if project => (text.to_owned(), InstructionSource::Global),
        _ => (default.to_owned(), InstructionSource::Default),
    };
    let own = own.filter(|text| !text.is_empty());
    let (text, source) = match &own {
        Some(text) if project => (text.clone(), InstructionSource::Project),
        Some(text) => (text.clone(), InstructionSource::Global),
        None => (inherited.clone(), inherited_source),
    };
    InstructionField {
        text,
        source,
        own,
        inherited,
        inherited_source,
    }
}

async fn global_values(conn: &mut PgConnection) -> Result<[Option<String>; 3]> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM server_state WHERE key = ANY($1)")
            .bind(&GLOBAL_KEYS[..])
            .fetch_all(conn)
            .await?;
    let mut out = [None, None, None];
    for (key, value) in rows {
        if let Some(i) = GLOBAL_KEYS.iter().position(|candidate| *candidate == key) {
            out[i] = Some(value);
        }
    }
    Ok(out)
}

async fn project_values(conn: &mut PgConnection, project_id: &str) -> Result<[Option<String>; 3]> {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT session_instruction, stop_nudge FROM projects WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(conn)
    .await?;
    let (a, b) = row.ok_or_else(|| StoreError::NotFound("project".into()))?;
    Ok([a, b, None])
}

async fn view(conn: &mut PgConnection, project_id: Option<&str>) -> Result<AgentInstructions> {
    let global = global_values(conn).await?;
    let own = match project_id {
        Some(id) => project_values(conn, id).await?,
        None => global.clone(),
    };
    let build = |i: usize| {
        field(
            own[i].clone(),
            global[i].as_deref(),
            default_text(i),
            project_id.is_some(),
        )
    };
    Ok(AgentInstructions {
        session_instruction: build(0),
        stop_nudge: build(1),
        chat_instruction: project_id.is_none().then(|| build(2)),
        max_bytes: MAX_INSTRUCTION_BYTES,
    })
}

impl Store {
    pub async fn chat_instruction(&self) -> Result<String> {
        let own: Option<String> =
            sqlx::query_scalar("SELECT value FROM server_state WHERE key = $1")
                .bind(GLOBAL_KEYS[2])
                .fetch_optional(&self.pool)
                .await?;
        Ok(own
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| DEFAULT_CHAT_INSTRUCTION.to_owned()))
    }

    /// The global instructions, or a project's with its overrides. A project the person is not a
    /// member of (an admin sees every project) is not found.
    pub async fn agent_instructions(
        &self,
        person_id: &str,
        is_admin: bool,
        project_id: Option<&str>,
    ) -> Result<AgentInstructions> {
        let mut conn = self.pool.acquire().await?;
        if let Some(id) = project_id
            && !is_admin
        {
            let member: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM project_members WHERE person_id = $1 AND project_id = $2)",
            )
            .bind(person_id)
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
            if !member {
                return Err(StoreError::NotFound("project".into()));
            }
        }
        view(&mut conn, project_id).await
    }

    /// The texts the hook gets with the session brief: what applies to the person's project (or
    /// globally without one, or for a project they are not a member of).
    pub async fn effective_instructions(
        &self,
        person_id: &str,
        project_id: Option<&str>,
    ) -> Result<AgentInstructions> {
        let mut conn = self.pool.acquire().await?;
        let member = match project_id {
            Some(id) => {
                sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM project_members WHERE person_id = $1 AND project_id = $2)",
                )
                .bind(person_id)
                .bind(id)
                .fetch_one(&mut *conn)
                .await?
            }
            None => false,
        };
        view(&mut conn, project_id.filter(|_| member)).await
    }

    /// Saves the given fields at the global scope (a server admin only) or for one project (its
    /// owner or a server admin). Each field is checked first; one that changes writes an audit
    /// event with the old and new text. Returns the instructions as they are now.
    pub async fn set_agent_instructions(
        &self,
        person_id: &str,
        is_admin: bool,
        device_id: Option<&str>,
        project_id: Option<&str>,
        update: &InstructionsUpdate,
    ) -> Result<AgentInstructions> {
        if project_id.is_some() && update.chat_instruction.is_some() {
            return Err(StoreError::Validation(
                "the chat instruction is global".into(),
            ));
        }
        let changes: Vec<(usize, String)> = [
            &update.session_instruction,
            &update.stop_nudge,
            &update.chat_instruction,
        ]
        .into_iter()
        .enumerate()
        .filter_map(|(i, text)| text.as_ref().map(|text| (i, text)))
        .map(|(i, text)| {
            validate_instruction(FIELDS[i], text)
                .map(|text| (i, text))
                .map_err(StoreError::Validation)
        })
        .collect::<Result<_>>()?;
        let mut tx = self.pool.begin().await?;
        match project_id {
            None if !is_admin => return Err(StoreError::Forbidden(ADMIN_ONLY_ERROR.into())),
            Some(id) => {
                let owner: String = sqlx::query_scalar(
                    "SELECT owner_id FROM projects WHERE project_id = $1 FOR UPDATE",
                )
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| StoreError::NotFound("project".into()))?;
                if owner != person_id && !is_admin {
                    return Err(StoreError::Forbidden(
                        "only the project's owner or a server admin can change its agent instructions"
                            .into(),
                    ));
                }
            }
            None => {}
        }
        let before = match project_id {
            Some(id) => project_values(&mut tx, id).await?,
            None => global_values(&mut tx).await?,
        };
        for (i, new) in changes {
            let old = before[i].clone().unwrap_or_default();
            if old == new {
                continue;
            }
            match project_id {
                Some(id) => {
                    sqlx::query(&format!(
                        "UPDATE projects SET {} = NULLIF($2, '') WHERE project_id = $1",
                        FIELDS[i]
                    ))
                    .bind(id)
                    .bind(&new)
                    .execute(&mut *tx)
                    .await?;
                }
                None if new.is_empty() => {
                    sqlx::query("DELETE FROM server_state WHERE key = $1")
                        .bind(GLOBAL_KEYS[i])
                        .execute(&mut *tx)
                        .await?;
                }
                None => {
                    sqlx::query(
                        "INSERT INTO server_state (key, value) VALUES ($1, $2)
                         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
                    )
                    .bind(GLOBAL_KEYS[i])
                    .bind(&new)
                    .execute(&mut *tx)
                    .await?;
                }
            }
            audit_tx(
                &mut tx,
                person_id,
                device_id,
                INSTRUCTIONS_AUDIT,
                project_id,
                serde_json::json!({
                    "scope": if project_id.is_some() { "project" } else { "global" },
                    "field": FIELDS[i],
                    "old": old,
                    "new": new,
                }),
            )
            .await?;
        }
        let now = view(&mut tx, project_id).await?;
        tx.commit().await?;
        Ok(now)
    }
}
