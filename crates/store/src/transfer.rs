//! Moving a person's own memories out and back in: the console's Export and Import. Import goes
//! through the normal agent save (quality checks, redaction, dedupe by identity hash).

use crate::memory::{MEMORY_SELECT_WITH_DETAIL, SaveSource, visible_with_archive};
use crate::{Result, Store, StoreError};
use cairn_core::capture::{ChatHash, WorkContext};
use cairn_core::id::now_seconds;
use cairn_core::memory::{Fields, Kind, Link, SaveInput, Scope, Status};
use cairn_core::process::{Origin, prepare_save};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashSet;

/// Most memories one import call takes; bigger files are split by the person.
pub const IMPORT_MAX: usize = 5_000;

#[derive(Clone, Debug, Serialize)]
pub struct ExportedMemory {
    pub id: String,
    pub kind: Kind,
    pub scope: Scope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_id_hash: Option<ChatHash>,
    pub status: Status,
    pub title: String,
    pub summary: String,
    pub reason: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    pub fields: Fields,
    pub evidence: Vec<String>,
    pub links: Vec<Link>,
    pub project_id: Option<String>,
    pub repository: Option<String>,
    pub branch: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct OwnedExport {
    pub version: u32,
    pub exported_at: i64,
    pub memories: Vec<ExportedMemory>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, Serialize)]
pub struct Invalid {
    pub index: usize,
    pub error: String,
}

/// `new` would be (or was) written, `duplicate` already exists, `invalid` says why not.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, Default, Serialize)]
pub struct ImportReport {
    pub new: usize,
    pub duplicate: usize,
    pub invalid: Vec<Invalid>,
}

/// The keys an export carries that a save takes; the rest (id, status, links, times) are ignored.
const SAVE_KEYS: [&str; 8] = [
    "kind", "title", "summary", "reason", "detail", "fields", "evidence", "scope",
];

impl Store {
    /// The memories the person owns and has not forgotten or lost to expiry, handoffs excluded
    /// (they are system notes about a checkout, not knowledge). Links are those whose other end
    /// the person can see.
    pub async fn export_owned(&self, person_id: &str) -> Result<OwnedExport> {
        let rows = sqlx::query(&format!(
            "{MEMORY_SELECT_WITH_DETAIL}
             WHERE m.owner_id = $1 AND {} AND m.kind <> 'handoff'
             ORDER BY m.created_at, m.memory_id",
            visible_with_archive("m", "$1", true)
        ))
        .bind(person_id)
        .fetch_all(&self.pool)
        .await?;
        let mut memories = rows
            .iter()
            .map(crate::memory::memory_from_row)
            .collect::<Result<Vec<_>>>()?;
        self.attach(person_id, &mut memories, true).await?;
        Ok(OwnedExport {
            version: 1,
            exported_at: now_seconds(),
            memories: memories
                .into_iter()
                .map(|m| ExportedMemory {
                    id: m.id,
                    kind: m.kind,
                    scope: m.scope,
                    chat_id_hash: m.chat_id_hash,
                    status: m.status,
                    title: m.title,
                    summary: m.summary,
                    reason: m.reason,
                    detail: m.detail,
                    fields: m.fields,
                    evidence: m.evidence,
                    links: m.links,
                    project_id: m.project_id,
                    repository: m.context.repo,
                    branch: m.context.branch,
                    created_at: m.created_at,
                    updated_at: m.updated_at,
                })
                .collect(),
        })
    }

    /// Reads each item as an agent save. A dry run only judges (and writes nothing); a real run
    /// saves what is new. A project memory is kept only for a project the person already belongs
    /// to: an import never joins a project.
    pub async fn import_memories(
        &self,
        person_id: &str,
        items: &[Value],
        dry_run: bool,
    ) -> Result<ImportReport> {
        if items.len() > IMPORT_MAX {
            return Err(StoreError::Validation(format!(
                "at most {IMPORT_MAX} memories per import"
            )));
        }
        let mut report = ImportReport::default();
        let mut seen = HashSet::new();
        for (index, item) in items.iter().enumerate() {
            match self.import_one(person_id, item, dry_run, &mut seen).await {
                Ok(true) => report.new += 1,
                Ok(false) => report.duplicate += 1,
                Err(error) => match error {
                    StoreError::Validation(m)
                    | StoreError::Quality(m)
                    | StoreError::Forbidden(m) => {
                        report.invalid.push(Invalid { index, error: m });
                    }
                    StoreError::Conflict(_) => report.invalid.push(Invalid {
                        index,
                        error: "you forgot this memory earlier; it stays forgotten".into(),
                    }),
                    other => return Err(other),
                },
            }
        }
        Ok(report)
    }

    /// True when the item is new, false when it already exists.
    async fn import_one(
        &self,
        person_id: &str,
        item: &Value,
        dry_run: bool,
        seen: &mut HashSet<(String, String, String)>,
    ) -> Result<bool> {
        let object = item
            .as_object()
            .ok_or_else(|| StoreError::Validation("each memory must be an object".into()))?;
        let mut keep = Map::new();
        for key in SAVE_KEYS {
            if let Some(value) = object.get(key).filter(|v| !v.is_null()) {
                keep.insert(key.to_owned(), value.clone());
            }
        }
        let input: SaveInput = serde_json::from_value(Value::Object(keep))
            .map_err(|e| StoreError::Validation(e.to_string()))?;
        let chat: Option<ChatHash> = object
            .get("chat_id_hash")
            .filter(|value| !value.is_null())
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
            .map_err(|error| StoreError::Validation(error.to_string()))?;
        if chat.is_some() && input.scope != Some(Scope::Personal) {
            return Err(StoreError::Validation(
                "chat-tagged memories must be personal".into(),
            ));
        }
        let wants_project = input.scope == Some(Scope::Project);
        let mut work = WorkContext::default();
        if wants_project {
            let project = object
                .get("project_id")
                .and_then(Value::as_str)
                .filter(|p| !p.is_empty())
                .ok_or_else(|| {
                    StoreError::Validation("a project memory needs project_id".into())
                })?;
            if !self.is_member(person_id, project).await? {
                return Err(StoreError::Validation(format!(
                    "you are not a member of project {project}"
                )));
            }
            work.project_id = Some(project.to_owned());
        }
        work.repository = object
            .get("repository")
            .and_then(Value::as_str)
            .map(str::to_owned);
        work.branch = object
            .get("branch")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if !dry_run {
            let source = chat.map_or(SaveSource::StrictAgent, SaveSource::ImportedChat);
            let saved = self
                .save_as(person_id, input, &work, source, None, None)
                .await?;
            return Ok(!saved.deduplicated);
        }
        let mut new = prepare_save(input, &work, wants_project, Origin::Agent, now_seconds())?;
        if let Some(hash) = chat {
            new.tag_chat(hash, true);
        }
        let key = (
            new.scope.as_str().to_owned(),
            work.project_id.clone().unwrap_or_default(),
            new.identity_hash.clone(),
        );
        if !seen.insert(key.clone()) {
            return Ok(false);
        }
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM memories
              WHERE owner_id = $1 AND scope = $2 AND coalesce(project_id, '') = $3 AND identity_hash = $4",
        )
        .bind(person_id)
        .bind(&key.0)
        .bind(&key.1)
        .bind(&key.2)
        .fetch_optional(&self.pool)
        .await?;
        match status.as_deref() {
            Some("forgotten") => Err(StoreError::Conflict("forgotten".into())),
            Some(_) => Ok(false),
            None => Ok(true),
        }
    }
}
