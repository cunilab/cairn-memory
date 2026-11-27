//! What the console shows of a memory: the stored memory plus where it came from, how it is used,
//! why it is stale and what it is linked to. Agents never see this shape (`Record` is theirs).

use crate::{Result, Store, epoch};
use cairn_core::memory::{Link, Memory, Relation, TrustLevel};
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
                let owner_is_me = memory.owner == person_id;
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
                MemoryView {
                    origin: memory.origin,
                    owner_is_me,
                    project_name,
                    project_archived,
                    pinned_at: memory.pinned_at,
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
