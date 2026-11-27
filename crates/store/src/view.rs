//! What the console shows of a memory: the stored memory plus where it came from, how it is used,
//! why it is stale and what it is linked to. Agents never see this shape (`Record` is theirs).

use crate::memory::{account_is_admin, may_edit_pin};
use crate::{Result, Store, epoch};
use cairn_core::memory::{Link, Memory, Relation, Scope, Status, TrustLevel};
use cairn_core::process::Origin;
use serde::Serialize;
use sqlx::Row;
use std::collections::HashMap;

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, Serialize)]
pub struct UsageView {
    pub fetched: u32,
    pub shown: u32,
    pub pushed: u32,
    pub push_used: u32,
    pub detail_opened: u32,
    pub helpful: u32,
    pub wrong: u32,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub last_used_at: Option<i64>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub last_shown_at: Option<i64>,
}

/// The memory at the other end of a link, as far as the reader may know it.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, Serialize)]
pub struct LinkOther {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub status: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, Serialize)]
pub struct LinkView {
    pub relation: Relation,
    pub source: String,
    pub target: String,
    pub auto: bool,
    /// `out`: this memory is the source. `in`: another memory links to it.
    #[cfg_attr(feature = "ts", ts(type = "\"out\" | \"in\""))]
    pub direction: &'static str,
    pub other: Option<LinkOther>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, Serialize)]
pub struct MemoryView {
    #[serde(flatten)]
    #[cfg_attr(feature = "ts", ts(flatten))]
    pub memory: Memory,
    pub origin: Origin,
    pub project_name: Option<String>,
    pub owner_is_me: bool,
    /// The device that saved it; only its owner sees the name.
    pub device_name: Option<String>,
    /// The last git check of a memory that cites files or symbols.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub checked_at: Option<i64>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub status_changed_at: i64,
    pub fetched: u32,
    pub usage: UsageView,
    /// Files and symbols the last check found gone: the "why stale" detail.
    pub missing_entities: Vec<String>,
    pub links: Vec<LinkView>,
    pub project_archived: bool,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub pinned_at: Option<i64>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub pin_position: Option<i32>,
    /// The viewer may pin or unpin this memory.
    pub can_pin: bool,
    /// The viewer may reorder the whole pin bucket this memory belongs to.
    pub can_reorder: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub provenance: Option<crate::provenance::MemoryProvenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub detail: Option<String>,
    pub trust_level: TrustLevel,
    pub trust: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PinSummary {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub personal_quota_used: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub personal_quota_limit: i64,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub project_quota_used: Option<i64>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub project_quota_limit: i64,
    pub personal_shown: usize,
    pub personal_total: usize,
    pub project_shown: usize,
    pub project_total: usize,
    pub shown: usize,
    pub total: usize,
    pub not_shown: usize,
    pub shown_ids: Vec<String>,
    pub omitted_ids: Vec<String>,
}

/// One pin bucket. Limits are the current personal (3) and project (4) quotas.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PinQuota {
    /// `personal` or `project`.
    pub scope: String,
    pub project_id: Option<String>,
    pub name: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub used: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub limit: i64,
}

struct Extra {
    project_name: Option<String>,
    project_archived: bool,
    device_name: Option<String>,
    checked_at: Option<i64>,
    status_changed_at: i64,
    missing: Vec<String>,
    pushed: u32,
    push_used: u32,
}

impl Store {
    /// The console shape of memories already read for `person_id` (links attached by `list`/`get`).
    pub async fn memory_views(
        &self,
        person_id: &str,
        memories: Vec<Memory>,
    ) -> Result<Vec<MemoryView>> {
        if memories.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<&str> = memories.iter().map(|m| m.id.as_str()).collect();
        let mut extras: HashMap<String, Extra> = HashMap::new();
        for row in sqlx::query(&format!(
            "SELECT m.memory_id, p.name AS project_name,
                    (m.scope = 'project' AND p.archived_at IS NOT NULL) AS project_archived,
                    CASE WHEN m.owner_id = $2 THEN d.name END AS device_name,
                    {} AS checked_at, {} AS status_changed_at, m.missing_entities,
                    coalesce(pu.pushed, 0) AS pushed, coalesce(pu.used, 0) AS push_used
               FROM memories m
               LEFT JOIN projects p ON p.project_id = m.project_id
               LEFT JOIN devices d ON d.device_id = m.device_id AND d.person_id = m.owner_id
               LEFT JOIN push_usage pu ON pu.person_id = $2 AND pu.memory_id = m.memory_id
              WHERE m.memory_id = ANY($1)",
            epoch!("m.checked_at"),
            epoch!("m.status_changed_at")
        ))
        .bind(&ids)
        .bind(person_id)
        .fetch_all(&self.pool)
        .await?
        {
            extras.insert(
                row.try_get("memory_id")?,
                Extra {
                    project_name: row.try_get("project_name")?,
                    project_archived: row.try_get("project_archived")?,
                    device_name: row.try_get("device_name")?,
                    checked_at: row.try_get("checked_at")?,
                    status_changed_at: row.try_get("status_changed_at")?,
                    missing: row.try_get("missing_entities")?,
                    pushed: row.try_get::<i32, _>("pushed")?.max(0) as u32,
                    push_used: row.try_get::<i32, _>("push_used")?.max(0) as u32,
                },
            );
        }
        let own: std::collections::HashSet<&str> = ids.iter().copied().collect();
        let others: Vec<&str> = memories
            .iter()
            .flat_map(|m| m.links.iter())
            .flat_map(|l| [l.source.as_str(), l.target.as_str()])
            .filter(|id| !own.contains(id))
            .collect();
        let mut named: HashMap<String, LinkOther> = HashMap::new();
        // The links were already limited to ends the reader can see (`attach`).
        for row in sqlx::query(
            "SELECT memory_id, kind, title, status FROM memories WHERE memory_id = ANY($1)",
        )
        .bind(&others)
        .fetch_all(&self.pool)
        .await?
        {
            let id: String = row.try_get("memory_id")?;
            named.insert(
                id.clone(),
                LinkOther {
                    id,
                    kind: row.try_get("kind")?,
                    title: row.try_get("title")?,
                    status: row.try_get("status")?,
                },
            );
        }
        // Both ends may be in the page: their own rows name each other.
        for m in &memories {
            named.entry(m.id.clone()).or_insert_with(|| LinkOther {
                id: m.id.clone(),
                kind: m.kind.as_str().to_owned(),
                title: m.title.clone(),
                status: m.status.as_str().to_owned(),
            });
        }
        let is_admin = account_is_admin(&self.pool, person_id).await?;
        let project_ids: Vec<&str> = memories
            .iter()
            .filter(|memory| memory.scope == Scope::Project)
            .filter_map(|memory| memory.project_id.as_deref())
            .collect();
        let mut project_owner: HashMap<String, String> = HashMap::new();
        for row in
            sqlx::query("SELECT project_id, owner_id FROM projects WHERE project_id = ANY($1)")
                .bind(&project_ids)
                .fetch_all(&self.pool)
                .await?
        {
            project_owner.insert(row.try_get("project_id")?, row.try_get("owner_id")?);
        }
        let member_of: std::collections::HashSet<String> = sqlx::query_scalar(
            "SELECT project_id FROM project_members WHERE person_id = $1 AND project_id = ANY($2)",
        )
        .bind(person_id)
        .bind(&project_ids)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .collect();
        let reorder_rows = sqlx::query(
            "SELECT m.project_id,
                    bool_and(m.owner_id = $1 OR p.owner_id = $1 OR $2) AS allowed
               FROM memories m
               JOIN projects p ON p.project_id = m.project_id
              WHERE m.scope = 'project' AND m.pinned_at IS NOT NULL
                AND m.project_id = ANY($3) AND p.archived_at IS NULL
              GROUP BY m.project_id",
        )
        .bind(person_id)
        .bind(is_admin)
        .bind(&project_ids)
        .fetch_all(&self.pool)
        .await?;
        let mut can_reorder_project: HashMap<String, bool> = HashMap::new();
        for row in reorder_rows {
            can_reorder_project.insert(row.try_get("project_id")?, row.try_get("allowed")?);
        }
        let mut provenance: HashMap<String, crate::provenance::MemoryProvenance> = HashMap::new();
        for row in sqlx::query(
            "SELECT ms.memory_id, ms.person_id, ms.session_id, ms.role, s.project_id,
                    s.context->>'branch' AS branch,
                    EXTRACT(EPOCH FROM s.started_at)::bigint AS started_at
               FROM memory_sessions ms
               JOIN sessions s ON s.person_id = ms.person_id AND s.session_id = ms.session_id
              WHERE ms.memory_id = ANY($1)
                AND (
                    s.person_id = $2
                    OR (
                        s.project_id IS NOT NULL
                        AND NOT (s.context ? 'chat')
                        AND EXISTS (
                            SELECT 1 FROM project_members pm
                             WHERE pm.project_id = s.project_id AND pm.person_id = $2
                        )
                    )
                )",
        )
        .bind(&ids)
        .bind(person_id)
        .fetch_all(&self.pool)
        .await?
        {
            let id: String = row.try_get("memory_id")?;
            provenance.insert(
                id,
                crate::provenance::MemoryProvenance {
                    person_id: row.try_get("person_id")?,
                    session_id: row.try_get("session_id")?,
                    project_id: row.try_get("project_id")?,
                    branch: row.try_get("branch")?,
                    started_at: row.try_get("started_at")?,
                    role: row.try_get("role")?,
                },
            );
        }
        let link_view = |memory: &Memory, link: &Link| {
            let out = link.source == memory.id;
            let other = if out { &link.target } else { &link.source };
            LinkView {
                relation: link.relation,
                source: link.source.clone(),
                target: link.target.clone(),
                auto: link.auto,
                direction: if out { "out" } else { "in" },
                other: named.get(other).cloned(),
            }
        };
        Ok(memories
            .into_iter()
            .map(|mut memory| {
                let owner_id = memory.owner.clone();
                let owner_is_me = owner_id == person_id;
                // Another person's id is of no use to the console.
                if !owner_is_me {
                    memory.owner.clear();
                }
                let extra = extras.remove(&memory.id);
                let (pushed, push_used) =
                    extra.as_ref().map_or((0, 0), |e| (e.pushed, e.push_used));
                let (project_name, project_archived, device_name, checked_at, changed, missing) =
                    match extra {
                        Some(e) => (
                            e.project_name,
                            e.project_archived,
                            e.device_name,
                            e.checked_at,
                            e.status_changed_at,
                            e.missing,
                        ),
                        None => (None, false, None, None, memory.updated_at, Vec::new()),
                    };
                let (trust_level, trust) = memory.trust();
                let project_key = memory.project_id.clone();
                let can_pin = may_edit_pin(
                    person_id,
                    is_admin,
                    memory.scope,
                    &owner_id,
                    project_key
                        .as_ref()
                        .and_then(|id| project_owner.get(id))
                        .map(String::as_str),
                    project_archived,
                    project_key
                        .as_ref()
                        .is_some_and(|id| member_of.contains(id)),
                ) && memory.status != Status::Forgotten
                    && (memory.status != Status::Superseded || memory.pinned_at.is_some());
                let can_reorder = memory.pinned_at.is_some()
                    && match memory.scope {
                        Scope::Personal => owner_is_me,
                        Scope::Project => project_key
                            .as_ref()
                            .and_then(|id| can_reorder_project.get(id).copied())
                            .unwrap_or(false),
                        Scope::Global => false,
                    };
                let provenance = provenance.remove(&memory.id);
                MemoryView {
                    origin: memory.origin,
                    owner_is_me,
                    project_name,
                    project_archived,
                    pinned_at: memory.pinned_at,
                    pin_position: memory.pin_position,
                    can_pin,
                    can_reorder,
                    provenance,
                    device_name,
                    checked_at,
                    status_changed_at: changed,
                    fetched: memory.usage.fetched,
                    trust_level,
                    trust,
                    usage: UsageView {
                        fetched: memory.usage.fetched,
                        shown: memory.usage.shown,
                        pushed,
                        push_used,
                        detail_opened: memory.usage.detail_opened,
                        helpful: memory.usage.helpful,
                        wrong: memory.usage.wrong,
                        last_used_at: memory.usage.last_used_at,
                        last_shown_at: memory.usage.last_shown_at,
                    },
                    missing_entities: missing,
                    links: memory.links.iter().map(|l| link_view(&memory, l)).collect(),
                    memory,
                    detail: None,
                }
            })
            .collect())
    }
}
