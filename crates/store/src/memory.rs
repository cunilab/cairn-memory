use crate::jobs::enqueue_tx;
use crate::project::join_member;
use crate::{Result, Store, StoreError};
use cairn_core::capture::WorkContext;
use cairn_core::id::{new_id, now_seconds};
use cairn_core::memory::coaching::save_hints;
use cairn_core::memory::model::Rank;
use cairn_core::memory::{
    Context, Entity, EntityKind, Fields, Kind, Link, Memory, Relation, SaveInput, Scope, Status,
    TrustLevel, Usage,
};
use cairn_core::process::jobs::JobKind;
use cairn_core::process::prepare::prepare_coached_save;
use cairn_core::process::{NewMemory, Origin, prepare_save};
use cairn_core::retrieve::rank::status_after_votes;
use sqlx::postgres::PgRow;
use sqlx::{Executor, PgConnection, Postgres, Row};
use std::collections::{HashMap, HashSet};

fn pin_limit(scope: Scope) -> Option<i64> {
    match scope {
        Scope::Personal => Some(5),
        Scope::Project => Some(10),
        Scope::Global => None,
    }
}

fn owned(alias: &str, me: &str) -> String {
    format!(
        "({a}.owner_id = {me} OR {a}.scope = 'global'
          OR ({a}.scope = 'project' AND {a}.project_id IN
               (SELECT project_id FROM project_members WHERE person_id = {me})))",
        a = alias
    )
}

/// Archive applies only to project scope, even when the reader owns the memory.
pub(crate) fn active_project_memory(alias: &str) -> String {
    format!(
        "({alias}.scope <> 'project' OR NOT EXISTS
            (SELECT 1 FROM projects p WHERE p.project_id = {alias}.project_id AND p.archived_at IS NOT NULL))"
    )
}

pub(crate) fn visible(alias: &str, me: &str) -> String {
    visible_with_archive(alias, me, false)
}

pub(crate) fn visible_with_archive(alias: &str, me: &str, include_archived: bool) -> String {
    let owned = owned(alias, me);
    let active = if include_archived {
        "TRUE".to_owned()
    } else {
        active_project_memory(alias)
    };
    format!("({alias}.status <> 'forgotten' AND {owned} AND {active})")
}

/// Search uses the caller's project ids loaded once and the shared archive predicate.
pub(crate) fn searchable(alias: &str, me: &str, projects: &str) -> String {
    let active = active_project_memory(alias);
    format!(
        "({a}.status IN ('active','stale','superseded')
          AND {active}
          AND ({a}.owner_id = {me} OR {a}.scope = 'global'
               OR ({a}.scope = 'project' AND {a}.project_id = ANY({projects}))))",
        a = alias
    )
}

/// Same as `visible`. The name stays so callers that used to include expired rows keep compiling.
pub(crate) fn visible_even_if_expired(alias: &str, me: &str) -> String {
    visible(alias, me)
}

macro_rules! memory_columns {
    ($detail:literal) => {
        concat!(
            "SELECT m.memory_id, m.owner_id, m.project_id, m.scope, m.kind,
    m.title, m.summary, m.reason, ", $detail, " AS detail, m.fields, m.evidence, m.evidence_count, m.evidence_hash,
    m.identity_hash, m.chat_id_hash, m.repo, m.repo_branch, m.repo_worktree, m.repo_commit, m.status, m.rank,
    m.proof_count, EXTRACT(EPOCH FROM m.faded_at)::bigint AS faded_at, (m.edited_by IS NOT NULL) AS edited_by,
    m.origin, m.stale_cause,
    EXTRACT(EPOCH FROM m.pinned_at)::bigint AS pinned_at,
    EXTRACT(EPOCH FROM m.expires_at)::bigint AS expires_at,
    EXTRACT(EPOCH FROM m.created_at)::bigint AS created_at,
    (EXTRACT(EPOCH FROM m.created_at) * 1000000)::bigint AS created_us,
    EXTRACT(EPOCH FROM m.updated_at)::bigint AS updated_at,
    to_char(m.updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS cursor_ts,
    COALESCE(u.fetched, 0) AS fetched, COALESCE(u.helpful, 0) AS helpful,
    COALESCE(u.wrong, 0) AS wrong, EXTRACT(EPOCH FROM u.last_used_at)::bigint AS last_used_at,
    COALESCE(u.shown, 0) AS shown, COALESCE(u.detail_opened, 0) AS detail_opened,
    EXTRACT(EPOCH FROM u.last_shown_at)::bigint AS last_shown_at")
    };
}

/// The columns of `memory_from_row`, for a caller that adds its own columns and joins.
pub(crate) const MEMORY_COLUMNS: &str = memory_columns!("''");
pub(crate) const MEMORY_SELECT: &str = concat!(
    memory_columns!("''"),
    " FROM memories m LEFT JOIN usage u ON u.memory_id = m.memory_id"
);
pub(crate) const MEMORY_SELECT_WITH_DETAIL: &str = concat!(
    memory_columns!("m.detail"),
    " FROM memories m LEFT JOIN usage u ON u.memory_id = m.memory_id"
);

pub(crate) const SEARCH_VECTOR: &str = "(m.search_tsv || to_tsvector('english', m.detail))";

/// Uses the original GIN for old rows and the partial GIN for detail-bearing rows.
pub(crate) fn search_matches(terms: &str) -> String {
    format!(
        "((m.detail = '' AND m.search_tsv @@ {terms})
           OR (m.detail <> '' AND {SEARCH_VECTOR} @@ {terms}))"
    )
}

pub(crate) async fn entities_for<'e>(
    executor: impl Executor<'e, Database = Postgres>,
    ids: &[String],
) -> Result<HashMap<String, Vec<Entity>>> {
    let mut entities: HashMap<String, Vec<Entity>> = HashMap::new();
    for row in sqlx::query(
        "SELECT memory_id, kind, name FROM entities WHERE memory_id = ANY($1) ORDER BY kind, name",
    )
    .bind(ids)
    .fetch_all(executor)
    .await?
    {
        let kind: EntityKind = parse(&row, "kind")?;
        entities
            .entry(row.try_get("memory_id")?)
            .or_default()
            .push(Entity {
                kind,
                name: row.try_get("name")?,
            });
    }
    Ok(entities)
}

fn parse<T: std::str::FromStr<Err = String>>(row: &PgRow, column: &str) -> Result<T> {
    row.try_get::<String, _>(column)?
        .parse()
        .map_err(StoreError::Validation)
}

/// Entities and links are attached separately by `attach`.
pub(crate) fn memory_from_row(row: &PgRow) -> Result<Memory> {
    let kind: Kind = parse(row, "kind")?;
    let fields = Fields::parse(kind, Some(row.try_get("fields")?))?;
    Ok(Memory {
        id: row.try_get("memory_id")?,
        owner: row.try_get("owner_id")?,
        project_id: row.try_get("project_id")?,
        scope: parse(row, "scope")?,
        kind,
        title: row.try_get("title")?,
        summary: row.try_get("summary")?,
        reason: row.try_get("reason")?,
        detail: row.try_get("detail")?,
        fields,
        evidence: row.try_get("evidence")?,
        evidence_count: row.try_get::<i32, _>("evidence_count")?.max(0) as u32,
        context: Context {
            repo: row.try_get("repo")?,
            branch: row.try_get("repo_branch")?,
            worktree: row.try_get("repo_worktree")?,
            commit: row.try_get("repo_commit")?,
        },
        entities: Vec::new(),
        links: Vec::new(),
        status: parse(row, "status")?,
        rank: parse(row, "rank")?,
        faded_at: row.try_get("faded_at")?,
        pinned_at: row.try_get("pinned_at")?,
        edited_by: row.try_get("edited_by")?,
        proof_count: row.try_get::<i32, _>("proof_count")?.max(0) as u32,
        stale_cause: row.try_get("stale_cause")?,
        origin: parse(row, "origin")?,
        expires_at: row.try_get("expires_at")?,
        created_at: row.try_get("created_at")?,
        created_us: row.try_get("created_us")?,
        updated_at: row.try_get("updated_at")?,
        evidence_hash: row.try_get("evidence_hash")?,
        identity_hash: row.try_get("identity_hash")?,
        chat_id_hash: row
            .try_get::<Option<String>, _>("chat_id_hash")?
            .map(cairn_core::capture::ChatHash::try_from)
            .transpose()
            .map_err(|error| StoreError::Validation(error.into()))?,
        usage: Usage {
            fetched: row.try_get::<i32, _>("fetched")?.max(0) as u32,
            shown: row.try_get::<i32, _>("shown")?.max(0) as u32,
            detail_opened: row.try_get::<i32, _>("detail_opened")?.max(0) as u32,
            helpful: row.try_get::<i32, _>("helpful")?.max(0) as u32,
            wrong: row.try_get::<i32, _>("wrong")?.max(0) as u32,
            last_used_at: row.try_get("last_used_at")?,
            last_shown_at: row.try_get("last_shown_at")?,
        },
    })
}

pub(crate) fn collection_memory(row: &PgRow) -> Result<Option<Memory>> {
    match memory_from_row(row) {
        Ok(memory) => Ok(Some(memory)),
        Err(StoreError::Validation(_)) => {
            let id: &str = row.try_get("memory_id")?;
            eprintln!("cairn: skipped malformed memory {id}");
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Which project a request attaches to, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectResolution {
    pub project_id: Option<String>,
    /// Set only for the repository fallback (no project id, exactly one membership).
    pub by_repository: bool,
}

impl ProjectResolution {
    fn personal() -> Self {
        Self {
            project_id: None,
            by_repository: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    pub id: String,
    pub status: Status,
    /// True when an identical memory already existed; nothing new was written.
    pub deduplicated: bool,
    pub hints: Vec<String>,
}

#[derive(Clone)]
pub(crate) enum SaveSource {
    Agent,
    StrictAgent,
    ImportedChat(cairn_core::capture::ChatHash),
    System,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vote {
    Helpful,
    Wrong,
}

#[derive(Clone, Debug, Default)]
pub struct ListFilter {
    pub project_id: Option<String>,
    pub scope: Option<Scope>,
    pub kind: Option<Kind>,
    /// Empty means every visible status.
    pub statuses: Vec<Status>,
    pub trust: Option<TrustLevel>,
    /// 1..=500, default 100.
    pub limit: Option<i64>,
    pub cursor: Option<String>,
    /// The branch the memory was saved on.
    pub branch: Option<String>,
    /// A web search over title, summary, reason, fields and detail. Never counts a fetch.
    pub q: Option<String>,
    /// An admin listing `forgotten` rows. Everyone else stays on `visible`.
    pub see_forgotten: bool,
    /// A console list filtered to an explicitly selected project.
    pub include_archived: bool,
    /// A device listing cannot browse chat-tagged personal memories.
    pub exclude_chat: bool,
}

#[derive(Clone, Debug)]
pub struct Page {
    pub memories: Vec<Memory>,
    pub next_cursor: Option<String>,
}

impl Store {
    /// A strictly validated agent save. Project membership comes from the work context.
    pub async fn save(
        &self,
        person_id: &str,
        input: SaveInput,
        work: &WorkContext,
    ) -> Result<Saved> {
        self.save_as(person_id, input, work, SaveSource::StrictAgent, None)
            .await
    }

    /// Explicit agent save with advisory coaching and optional device provenance.
    pub async fn save_from(
        &self,
        person_id: &str,
        input: SaveInput,
        work: &WorkContext,
        device_id: Option<&str>,
    ) -> Result<Saved> {
        self.save_as(person_id, input, work, SaveSource::Agent, device_id)
            .await
    }

    pub(crate) async fn save_as(
        &self,
        person_id: &str,
        input: SaveInput,
        work: &WorkContext,
        source: SaveSource,
        device_id: Option<&str>,
    ) -> Result<Saved> {
        let project_id = self.context_project(person_id, work, device_id).await?;
        let mut tx = self.pool.begin().await?;
        let saved = save_in_tx(
            &mut tx,
            person_id,
            input,
            work,
            source,
            device_id,
            project_id.as_deref(),
        )
        .await?;
        tx.commit().await?;
        Ok(saved)
    }

    /// The request's project when the person is (or by the committed file becomes) a member.
    /// With no project id, exactly one membership for a non-`local:` repository is that project.
    pub(crate) async fn context_project(
        &self,
        person_id: &str,
        work: &WorkContext,
        device_id: Option<&str>,
    ) -> Result<Option<String>> {
        if work.chat.is_some() {
            return Ok(self.resolve_project(person_id, work).await?.project_id);
        }
        if let Some(project_id) = work
            .project_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            let mut tx = self.pool.begin().await?;
            let (exists, inserted) = join_member(&mut tx, person_id, project_id).await?;
            if inserted {
                crate::account::audit_tx(&mut tx, person_id, device_id, "project_auto_joined", Some(project_id), serde_json::json!({"project_id":project_id,"person":person_id,"device":device_id,"source":"project_file"})).await?;
            }
            tx.commit().await?;
            return Ok(exists.then(|| project_id.to_owned()));
        }
        Ok(self.resolve_project(person_id, work).await?.project_id)
    }

    /// How a request attaches. An explicit project id counts only when the person is already a
    /// member (capture joins first). `by_repository` is the single-membership fallback.
    pub async fn resolve_project(
        &self,
        person_id: &str,
        work: &WorkContext,
    ) -> Result<ProjectResolution> {
        if let Some(project_id) = work
            .project_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            let member = self.is_member(person_id, project_id).await?;
            return Ok(ProjectResolution {
                project_id: member.then(|| project_id.to_owned()),
                by_repository: false,
            });
        }
        let Some(repository) = work
            .repository
            .as_deref()
            .map(str::trim)
            .filter(|repository| !repository.is_empty() && !repository.starts_with("local:"))
        else {
            return Ok(ProjectResolution::personal());
        };
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT p.project_id FROM projects p
              JOIN project_members m ON m.project_id = p.project_id
             WHERE p.repository = $1 AND m.person_id = $2
             LIMIT 2",
        )
        .bind(repository)
        .bind(person_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(match ids.as_slice() {
            [id] => ProjectResolution {
                project_id: Some(id.clone()),
                by_repository: true,
            },
            _ => ProjectResolution::personal(),
        })
    }

    pub async fn get(&self, person_id: &str, memory_id: &str) -> Result<Memory> {
        self.get_visible(person_id, memory_id, false).await
    }

    pub async fn get_visible(
        &self,
        person_id: &str,
        memory_id: &str,
        include_archived: bool,
    ) -> Result<Memory> {
        let row = sqlx::query(&format!(
            "{MEMORY_SELECT_WITH_DETAIL} WHERE m.memory_id = $2 AND {}",
            visible_with_archive("m", "$1", include_archived)
        ))
        .bind(person_id)
        .bind(memory_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("memory".into()))?;
        let mut memories = vec![memory_from_row(&row)?];
        self.attach(person_id, &mut memories, include_archived)
            .await?;
        Ok(memories.remove(0))
    }

    /// Counts an explicit fetch by an agent (`get` or search).
    pub async fn mark_fetched(&self, memory_ids: &[String]) -> Result<()> {
        self.mark_agent_read(memory_ids, false).await
    }

    /// A full-detail get is also a fetch. Count both in the same transaction.
    pub async fn mark_agent_read(&self, memory_ids: &[String], detail: bool) -> Result<()> {
        if memory_ids.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        let ids: Vec<&str> = memory_ids.iter().map(String::as_str).collect();
        match crate::project::lock_memory_projects(&mut tx, &ids).await {
            Err(StoreError::Conflict(_)) => return Ok(()),
            result => result?,
        }
        sqlx::query("UPDATE memories SET faded_at = NULL WHERE memory_id = ANY($1) AND faded_at IS NOT NULL")
            .bind(memory_ids)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO usage (memory_id, fetched, last_used_at, detail_opened)
             SELECT memory_id, 1, now(), $2 FROM memories WHERE memory_id = ANY($1) ORDER BY memory_id
             ON CONFLICT (memory_id) DO UPDATE SET fetched = usage.fetched + 1, last_used_at = now(),
                 detail_opened = usage.detail_opened + EXCLUDED.detail_opened",
        )
        .bind(memory_ids)
        .bind(i32::from(detail))
        .execute(&mut *tx)
        .await?;
        promote_candidates(&mut *tx, memory_ids, false).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Newest update first, paged by an opaque cursor. Only memories visible to the person.
    pub async fn list(&self, person_id: &str, filter: &ListFilter) -> Result<Page> {
        let limit = filter.limit.unwrap_or(100).clamp(1, 500);
        let (cursor_ts, cursor_id) = match filter.cursor.as_deref() {
            Some(cursor) => {
                let (ts, id) = cursor
                    .split_once('|')
                    .ok_or_else(|| StoreError::Validation("invalid cursor".into()))?;
                (Some(ts.to_owned()), Some(id.to_owned()))
            }
            None => (None, None),
        };
        let statuses: Vec<String> = filter
            .statuses
            .iter()
            .map(|s| s.as_str().to_owned())
            .collect();
        let include_archived = filter.include_archived && filter.project_id.is_some();
        let scope = if filter.see_forgotten {
            let owned = owned("m", "$1");
            if include_archived {
                owned
            } else {
                format!("{owned} AND {}", active_project_memory("m"))
            }
        } else {
            visible_with_archive("m", "$1", include_archived)
        };
        let q = filter.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
        let matches = search_matches("websearch_to_tsquery('english', $10)");
        // Match Memory::trust before LIMIT so the filter covers every page.
        let trust = "CASE
            WHEN m.status = 'stale' OR u.wrong > 0 THEN 'doubted'
            WHEN m.rank = 'candidate' THEN 'new'
            WHEN u.helpful >= 2 OR m.proof_count >= 3 THEN 'proven'
            ELSE 'normal'
        END";
        let rows = sqlx::query(&format!(
            "{MEMORY_SELECT}
             WHERE {scope} AND ($2::text IS NULL OR m.project_id = $2)
               AND ($3::text IS NULL OR m.scope = $3) AND ($4::text IS NULL OR m.kind = $4)
               AND (cardinality($5::text[]) = 0 OR m.status = ANY($5))
               AND ($6::text IS NULL OR (m.updated_at, m.memory_id) < ($6::timestamptz, $7::text))
               AND ($9::text IS NULL OR m.repo_branch = $9)
               AND ($10::text IS NULL OR {matches})
               AND ($11::text IS NULL OR ({trust}) = $11)
               AND (NOT $12 OR m.chat_id_hash IS NULL)
             ORDER BY m.updated_at DESC, m.memory_id DESC LIMIT $8"
        ))
        .bind(person_id)
        .bind(&filter.project_id)
        .bind(filter.scope.map(Scope::as_str))
        .bind(filter.kind.map(Kind::as_str))
        .bind(&statuses)
        .bind(cursor_ts)
        .bind(cursor_id)
        .bind(limit + 1)
        .bind(&filter.branch)
        .bind(q)
        .bind(filter.trust.map(TrustLevel::as_str))
        .bind(filter.exclude_chat)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| match &error {
            // 22007/22008/22P02: the cursor's timestamp does not parse.
            sqlx::Error::Database(e)
                if matches!(e.code().as_deref(), Some("22007" | "22008" | "22P02")) =>
            {
                StoreError::Validation("invalid cursor".into())
            }
            _ => error.into(),
        })?;
        let more = rows.len() as i64 > limit;
        let rows = &rows[..rows.len().min(limit as usize)];
        let next_cursor = match (more, rows.last()) {
            (true, Some(last)) => Some(format!(
                "{}|{}",
                last.try_get::<String, _>("cursor_ts")?,
                last.try_get::<String, _>("memory_id")?
            )),
            _ => None,
        };
        let mut memories = rows
            .iter()
            .filter_map(|row| collection_memory(row).transpose())
            .collect::<Result<Vec<_>>>()?;
        self.attach(person_id, &mut memories, include_archived)
            .await?;
        Ok(Page {
            memories,
            next_cursor,
        })
    }

    /// Fills `entities` and `links` (outgoing and incoming). A link whose other end the reader
    /// cannot see is left out, so links never reveal a hidden memory.
    pub(crate) async fn attach(
        &self,
        person_id: &str,
        memories: &mut [Memory],
        include_archived: bool,
    ) -> Result<()> {
        if memories.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = memories.iter().map(|m| m.id.clone()).collect();
        let mut entities = entities_for(&self.pool, &ids).await?;
        let link_rows = sqlx::query(
            "SELECT source_id, target_id, relation, origin FROM links
              WHERE source_id = ANY($1) OR target_id = ANY($1) ORDER BY created_at, source_id, target_id",
        )
        .bind(&ids)
        .fetch_all(&self.pool)
        .await?;
        let own: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let others: Vec<String> = link_rows
            .iter()
            .flat_map(|r| {
                [
                    r.get::<String, _>("source_id"),
                    r.get::<String, _>("target_id"),
                ]
            })
            .filter(|id| !own.contains(id.as_str()))
            .collect();
        let readable: HashSet<String> = sqlx::query_scalar(&format!(
            "SELECT m.memory_id FROM memories m WHERE m.memory_id = ANY($2) AND {}",
            visible_with_archive("m", "$1", include_archived)
        ))
        .bind(person_id)
        .bind(&others)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .collect();
        let mut links: HashMap<String, Vec<Link>> = HashMap::new();
        for row in &link_rows {
            let (source, target): (String, String) =
                (row.try_get("source_id")?, row.try_get("target_id")?);
            let link = Link {
                relation: parse(row, "relation")?,
                auto: row.try_get::<String, _>("origin")? == "auto",
                source: source.clone(),
                target: target.clone(),
            };
            for end in [&source, &target] {
                let other = if *end == source { &target } else { &source };
                if own.contains(end.as_str())
                    && (own.contains(other.as_str()) || readable.contains(other))
                {
                    links.entry(end.clone()).or_default().push(link.clone());
                }
            }
        }
        for memory in memories {
            memory.entities = entities.remove(&memory.id).unwrap_or_default();
            memory.links = links.remove(&memory.id).unwrap_or_default();
            // A local path is the owner's business; members and global readers do not get it.
            if memory.owner != person_id {
                memory.context.worktree = None;
            }
        }
        Ok(())
    }

    /// `helpful` or `wrong`. Feeds ranking and the stale rule. A person has one vote per memory: a
    /// new vote replaces their earlier one, so a `helpful` after a `wrong` takes the `wrong` back
    /// (and a memory it had made stale is active again).
    pub async fn feedback(&self, person_id: &str, memory_id: &str, vote: Vote) -> Result<Status> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(&format!(
            "SELECT m.owner_id, m.status, m.stale_cause, m.scope, m.project_id FROM memories m WHERE m.memory_id = $2 AND {} FOR UPDATE OF m",
            visible("m", "$1")
        ))
        .bind(person_id)
        .bind(memory_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StoreError::NotFound("memory".into()))?;
        lock_project_memory(&mut tx, parse(&row, "scope")?, row.try_get("project_id")?).await?;
        let owner: String = row.try_get("owner_id")?;
        let status: Status = parse(&row, "status")?;
        let by_feedback =
            row.try_get::<Option<String>, _>("stale_cause")?.as_deref() == Some("feedback");
        let vote_text = if vote == Vote::Helpful {
            "helpful"
        } else {
            "wrong"
        };
        sqlx::query(
            "DELETE FROM usage_votes WHERE memory_id = $1 AND person_id = $2 AND vote <> $3",
        )
        .bind(memory_id)
        .bind(person_id)
        .bind(vote_text)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO usage_votes (memory_id, person_id, vote) VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING",
        )
        .bind(memory_id)
        .bind(person_id)
        .bind(vote_text)
        .execute(&mut *tx)
        .await?;
        let counts = sqlx::query(
            "INSERT INTO usage (memory_id, helpful, wrong)
             SELECT $1, count(*) FILTER (WHERE vote = 'helpful'), count(*) FILTER (WHERE vote = 'wrong')
               FROM usage_votes WHERE memory_id = $1
             ON CONFLICT (memory_id) DO UPDATE SET helpful = EXCLUDED.helpful, wrong = EXCLUDED.wrong
             RETURNING wrong, EXISTS (SELECT 1 FROM usage_votes
                                       WHERE memory_id = $1 AND person_id = $2 AND vote = 'wrong') AS owner_wrong",
        )
        .bind(memory_id)
        .bind(&owner)
        .fetch_one(&mut *tx)
        .await?;
        let wrong = counts.try_get::<i32, _>("wrong")?.max(0) as u32;
        let new_status =
            status_after_votes(status, by_feedback, wrong, counts.try_get("owner_wrong")?);
        if new_status != status {
            sqlx::query(
                "UPDATE memories SET status = $2, stale_cause = CASE WHEN $2 = 'stale' THEN 'feedback' END,
                        status_changed_at = now(), updated_at = now() WHERE memory_id = $1",
            )
                .bind(memory_id)
                .bind(new_status.as_str())
                .execute(&mut *tx)
                .await?;
        }
        if vote == Vote::Helpful {
            let ids = [memory_id.to_owned()];
            promote_candidates(&mut *tx, &ids, true).await?;
        }
        tx.commit().await?;
        Ok(new_status)
    }

    /// Owner only. The row stays as `forgotten` with its text, fields, embedding, entities and
    /// links kept, so saving the same thing again is a 409. Pending jobs are cancelled. A memory
    /// this one superseded comes back unless another non-forgotten memory also supersedes it.
    /// Toggle the owner's pin. All writers in a quota bucket share a transaction lock.
    pub async fn set_pinned(
        &self,
        person_id: &str,
        memory_id: &str,
        pinned: bool,
    ) -> Result<Option<i64>> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT m.scope, m.project_id, m.status, EXTRACT(EPOCH FROM m.pinned_at)::bigint AS pinned_at
               FROM memories m WHERE m.memory_id = $1 AND m.owner_id = $2 AND m.status <> 'forgotten'
                 AND (m.scope <> 'project' OR EXISTS (
                     SELECT 1 FROM project_members pm
                     WHERE pm.project_id = m.project_id AND pm.person_id = $2))
               FOR UPDATE OF m",
        )
        .bind(memory_id)
        .bind(person_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StoreError::NotFound("memory".into()))?;
        let project_id: Option<String> = row.try_get("project_id")?;
        let scope = parse(&row, "scope")?;
        let limit = pin_limit(scope).ok_or_else(|| {
            StoreError::Validation("only personal and project memories can be pinned".into())
        })?;
        let bucket = match scope {
            Scope::Project => project_id
                .as_deref()
                .ok_or_else(|| StoreError::Validation("project memory has no project".into()))?,
            _ => person_id,
        };
        lock_project_memory(&mut tx, scope, project_id.as_deref()).await?;
        if pinned && parse::<Status>(&row, "status")? == Status::Superseded {
            return Err(StoreError::Validation(
                "superseded memories cannot be pinned".into(),
            ));
        }
        let old: Option<i64> = row.try_get("pinned_at")?;
        if old.is_some() == pinned {
            return Ok(old);
        }

        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtext(current_schema() || ':pins:' || $1), hashtext($2))",
        )
        .bind(scope.as_str())
        .bind(bucket)
        .execute(&mut *tx)
        .await?;
        if pinned {
            // Keep superseded pins in the quota so restoring a correction cannot exceed it.
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM memories WHERE pinned_at IS NOT NULL AND scope = $1
                   AND CASE WHEN $1 = 'project' THEN project_id = $2 ELSE owner_id = $2 END",
            )
            .bind(scope.as_str())
            .bind(bucket)
            .fetch_one(&mut *tx)
            .await?;
            if count >= limit {
                return Err(StoreError::Conflict(format!(
                    "{} pin limit of {limit} memories reached",
                    scope.as_str()
                )));
            }
        }
        let timestamp = sqlx::query_scalar(
            "UPDATE memories SET pinned_at = CASE WHEN $2 THEN now() END
               WHERE memory_id = $1 RETURNING EXTRACT(EPOCH FROM pinned_at)::bigint",
        )
        .bind(memory_id)
        .bind(pinned)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(timestamp)
    }

    /// True when this call forgot it, false when it already was forgotten.
    pub async fn forget(&self, person_id: &str, memory_id: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT status, scope, project_id FROM memories WHERE memory_id = $1 AND owner_id = $2 FOR UPDATE",
        )
        .bind(memory_id)
        .bind(person_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StoreError::NotFound("memory".into()))?;
        lock_project_memory(&mut tx, parse(&row, "scope")?, row.try_get("project_id")?).await?;
        if row.try_get::<String, _>("status")? == "forgotten" {
            return Ok(false);
        }
        let restored: Vec<String> = sqlx::query_scalar(
            "SELECT target_id FROM links WHERE source_id = $1 AND relation = 'supersedes'",
        )
        .bind(memory_id)
        .fetch_all(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE memories SET status = 'forgotten', pinned_at = NULL, status_changed_at = now(), updated_at = now()
              WHERE memory_id = $1",
        )
        .bind(memory_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM jobs WHERE kind IN ('embed','consolidate','link') AND job_key = $1 AND status = 'pending'")
            .bind(memory_id)
            .execute(&mut *tx)
            .await?;
        for target in restored {
            crate::project::lock_memory_projects(&mut tx, &[&target]).await?;
            restore_if_unsuperseded(&mut tx, &target).await?;
        }
        tx.commit().await?;
        Ok(true)
    }
}

/// Back to `active` when no remaining non-forgotten memory supersedes it.
pub(crate) async fn restore_if_unsuperseded(tx: &mut PgConnection, memory_id: &str) -> Result<()> {
    sqlx::query(
        "UPDATE memories SET status = 'active', status_changed_at = now(), updated_at = now()
          WHERE memory_id = $1 AND status = 'superseded'
            AND NOT EXISTS (SELECT 1 FROM links l JOIN memories s ON s.memory_id = l.source_id
                             WHERE l.target_id = $1 AND l.relation = 'supersedes' AND s.status <> 'forgotten')",
    )
    .bind(memory_id)
    .execute(tx)
    .await?;
    Ok(())
}

/// Writes one agent link. `supersedes` needs a target the person may replace: their own memory, or
/// another project memory when the new one is project-scoped in the same project.
pub(crate) async fn link_tx(
    tx: &mut PgConnection,
    person_id: &str,
    source: &str,
    source_scope: Scope,
    source_project: Option<&str>,
    target: &str,
    relation: Relation,
) -> Result<()> {
    if source == target {
        return Err(StoreError::Validation(
            "a memory cannot link to itself".into(),
        ));
    }
    let row = sqlx::query(&format!(
        "SELECT m.owner_id, m.scope, m.project_id FROM memories m WHERE m.memory_id = $2 AND {}
           AND m.chat_id_hash IS NOT DISTINCT FROM (SELECT chat_id_hash FROM memories WHERE memory_id = $3)",
        visible("m", "$1")
    ))
    .bind(person_id)
    .bind(target)
    .bind(source)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| StoreError::NotFound(format!("memory {target}")))?;
    lock_project_memory(tx, parse(&row, "scope")?, row.try_get("project_id")?).await?;
    if relation == Relation::Supersedes {
        let owner: String = row.try_get("owner_id")?;
        let same_project = parse::<Scope>(&row, "scope")? == Scope::Project
            && source_scope == Scope::Project
            && row.try_get::<Option<String>, _>("project_id")?.as_deref() == source_project;
        if owner != person_id && !same_project {
            return Err(StoreError::Forbidden(
                "cannot supersede another person's memory".into(),
            ));
        }
    }
    sqlx::query(
        "INSERT INTO links (source_id, target_id, relation, origin) VALUES ($1, $2, $3, 'agent')
         ON CONFLICT (source_id, target_id, relation) DO NOTHING",
    )
    .bind(source)
    .bind(target)
    .bind(relation.as_str())
    .execute(&mut *tx)
    .await?;
    if relation == Relation::Supersedes {
        sqlx::query("UPDATE memories SET status = 'superseded', stale_cause = NULL, status_changed_at = now(), updated_at = now() WHERE memory_id = $1 AND status IN ('active','stale')")
            .bind(target)
            .execute(tx)
            .await?;
    }
    Ok(())
}

/// A read confirms system candidates. Agent candidates need a correction or a helpful vote.
pub(crate) async fn promote_candidates<'e>(
    db: impl sqlx::Executor<'e, Database = sqlx::Postgres>,
    memory_ids: &[String],
    include_agent: bool,
) -> Result<()> {
    if memory_ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "UPDATE memories SET rank = 'normal'
          WHERE memory_id = ANY($1) AND rank = 'candidate' AND ($2 OR origin = 'system')",
    )
    .bind(memory_ids)
    .bind(include_agent)
    .execute(db)
    .await?;
    Ok(())
}

pub(crate) async fn save_in_tx(
    tx: &mut PgConnection,
    person_id: &str,
    mut input: SaveInput,
    work: &WorkContext,
    source: SaveSource,
    device_id: Option<&str>,
    project_id: Option<&str>,
) -> Result<Saved> {
    let aliases: Vec<String> = input
        .aliases
        .iter()
        .map(|alias| cairn_core::capture::redact_sensitive_content(alias.trim()))
        .collect();
    input.aliases = aliases.clone();
    let mut new = match &source {
        SaveSource::Agent => {
            prepare_coached_save(input, work, project_id.is_some(), now_seconds())?
        }
        SaveSource::StrictAgent | SaveSource::ImportedChat(_) => prepare_save(
            input,
            work,
            project_id.is_some(),
            Origin::Agent,
            now_seconds(),
        )?,
        SaveSource::System => prepare_save(
            input,
            work,
            project_id.is_some(),
            Origin::System,
            now_seconds(),
        )?,
    };
    if let SaveSource::ImportedChat(hash) = &source {
        new.tag_chat(hash.clone(), true);
    }
    let saved = insert_memory(
        tx,
        person_id,
        project_id,
        &new,
        None,
        device_id,
        matches!(source, SaveSource::Agent),
    )
    .await?;
    if !saved.deduplicated {
        insert_agent_aliases(tx, &saved.id, &aliases).await?;
    }
    Ok(saved)
}

async fn lock_project_memory(
    tx: &mut PgConnection,
    scope: Scope,
    project_id: Option<&str>,
) -> Result<()> {
    if scope == Scope::Project
        && let Some(id) = project_id
    {
        crate::project::lock_active_project(tx, id).await?;
    }
    Ok(())
}

/// Inserts a derived memory with its entities, usage row, agent links and embed job. An identical
/// memory (same owner, scope, project and identity hash) returns as a duplicate. A forgotten one
/// is a 409. `fixed_id` is for a caller that already chose the id.
pub(crate) async fn insert_memory(
    tx: &mut PgConnection,
    person_id: &str,
    project_id: Option<&str>,
    new: &NewMemory,
    fixed_id: Option<&str>,
    device_id: Option<&str>,
    coach: bool,
) -> Result<Saved> {
    lock_project_memory(tx, new.scope, project_id).await?;
    let project_id = if new.scope == Scope::Global || new.chat_id_hash.is_some() {
        None
    } else {
        project_id
    };
    let id = fixed_id.map_or_else(|| new_id("mem"), str::to_owned);
    let mut hints = if coach && new.kind != Kind::Handoff {
        let duplicate = crate::consolidate::save_duplicate(tx, person_id, project_id, new).await?;
        save_hints(
            &new.title,
            &new.summary,
            &new.reason,
            &new.fields,
            duplicate
                .as_ref()
                .map(|(id, title)| (id.as_str(), title.as_str())),
        )
    } else {
        Vec::new()
    };
    let rank = if new.chat_candidate
        || !hints.is_empty()
        || (new.origin == Origin::System && new.kind != Kind::Handoff && new.scope != Scope::Global)
    {
        Rank::Candidate
    } else {
        Rank::Normal
    };
    let inserted: Option<String> = sqlx::query_scalar(
        "INSERT INTO memories (memory_id, owner_id, project_id, scope, kind, title, summary, reason, fields,
                               evidence, evidence_count, evidence_hash, identity_hash,
                               repo, repo_branch, repo_worktree, repo_commit, expires_at, origin, device_id, rank, detail, chat_id_hash)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17, to_timestamp($18), $19, $20, $21, $22, $23)
         ON CONFLICT (owner_id, scope, (coalesce(project_id, '')), identity_hash) DO NOTHING
         RETURNING memory_id",
    )
    .bind(&id)
    .bind(person_id)
    .bind(project_id)
    .bind(new.scope.as_str())
    .bind(new.kind.as_str())
    .bind(&new.title)
    .bind(&new.summary)
    .bind(&new.reason)
    .bind(new.fields.to_json())
    .bind(&new.evidence)
    .bind(new.evidence.len() as i32)
    .bind(&new.evidence_hash)
    .bind(&new.identity_hash)
    .bind(&new.context.repo)
    .bind(&new.context.branch)
    .bind(&new.context.worktree)
    .bind(&new.context.commit)
    .bind(new.expires_at.map(|t| t as f64))
    .bind(new.origin.as_str())
    .bind(device_id)
    .bind(rank.as_str())
    .bind(new.detail.as_deref().unwrap_or_default())
    .bind(new.chat_id_hash.as_ref().map(|hash| hash.as_str()))
    .fetch_optional(&mut *tx)
    .await?;
    let Some(id) = inserted else {
        let existing = sqlx::query(
            "SELECT memory_id, status, origin, rank FROM memories
              WHERE owner_id = $1 AND scope = $2 AND coalesce(project_id, '') = coalesce($3, '') AND identity_hash = $4",
        )
        .bind(person_id)
        .bind(new.scope.as_str())
        .bind(project_id)
        .bind(&new.identity_hash)
        .fetch_one(&mut *tx)
        .await?;
        let status: Status = parse(&existing, "status")?;
        if status == Status::Forgotten {
            return Err(StoreError::Conflict("forgotten".into()));
        }
        let id: String = existing.try_get("memory_id")?;
        let coached = parse::<Origin>(&existing, "origin")? == Origin::Agent
            && parse::<Rank>(&existing, "rank")? == Rank::Candidate;
        let self_correction = new.supersedes.as_deref() == Some(id.as_str());
        if !new.chat_candidate
            && new.origin != Origin::System
            && (!coached || (self_correction && coach && hints.is_empty()))
        {
            promote_candidates(&mut *tx, std::slice::from_ref(&id), true).await?;
            sqlx::query("UPDATE memories SET proof_count = proof_count + 1 WHERE memory_id = $1")
                .bind(&id)
                .execute(&mut *tx)
                .await?;
        }
        // A correction to non-identifying text dedupes onto its own target.
        if self_correction {
            sqlx::query("UPDATE memories SET reason = $2, detail = coalesce($3, detail), updated_at = now() WHERE memory_id = $1")
                .bind(&id)
                .bind(&new.reason)
                .bind(&new.detail)
                .execute(&mut *tx)
                .await?;
        } else if let Some(detail) = new.detail.as_deref().filter(|detail| !detail.is_empty()) {
            sqlx::query("UPDATE memories SET detail = $2 WHERE memory_id = $1 AND detail = ''")
                .bind(&id)
                .bind(detail)
                .execute(&mut *tx)
                .await?;
        }
        if !coached {
            hints.clear();
        }
        save_links(tx, person_id, &id, project_id, new).await?;
        return Ok(Saved {
            id,
            status,
            deduplicated: true,
            hints,
        });
    };
    let (kinds, names): (Vec<&str>, Vec<&str>) = new
        .entities
        .iter()
        .map(|e| (e.kind.as_str(), e.name.as_str()))
        .unzip();
    sqlx::query("INSERT INTO entities (memory_id, kind, name) SELECT $1, k, n FROM unnest($2::text[], $3::text[]) AS t(k, n) ON CONFLICT DO NOTHING")
        .bind(&id)
        .bind(&kinds)
        .bind(&names)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO usage (memory_id) VALUES ($1)")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    save_links(tx, person_id, &id, project_id, new).await?;
    enqueue_tx(tx, JobKind::Embed, &id).await?;
    if new.kind != Kind::Handoff {
        enqueue_tx(tx, JobKind::Link, &id).await?;
    }
    Ok(Saved {
        id,
        status: Status::Active,
        deduplicated: false,
        hints,
    })
}

/// Agent aliases for a new memory, ords 0..4. A duplicate save leaves the stored rows as they are.
async fn insert_agent_aliases(
    tx: &mut PgConnection,
    memory_id: &str,
    aliases: &[String],
) -> Result<()> {
    for (ord, text) in aliases.iter().enumerate() {
        sqlx::query(
            "INSERT INTO memory_aliases (memory_id, ord, source, text) VALUES ($1, $2, 'agent', $3)",
        )
        .bind(memory_id)
        .bind(ord as i16)
        .bind(text)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

/// The `supersedes` and `contradicts` links a save carries; a duplicate save adds any it lacks.
async fn save_links(
    tx: &mut PgConnection,
    person_id: &str,
    id: &str,
    project_id: Option<&str>,
    new: &NewMemory,
) -> Result<()> {
    for (target, relation) in [
        (&new.supersedes, Relation::Supersedes),
        (&new.contradicts, Relation::Contradicts),
    ] {
        // Only a deduplicated save can name itself; there is nothing to link then.
        if let Some(target) = target.as_ref().filter(|t| t.as_str() != id) {
            link_tx(tx, person_id, id, new.scope, project_id, target, relation).await?;
        }
    }
    Ok(())
}

impl Store {
    /// Admin edit retains the owner and provenance, and atomically audits the successor.
    pub async fn rewrite_memory(
        &self,
        admin: &str,
        id: &str,
        title: String,
        summary: String,
        reason: String,
        fields: Option<serde_json::Value>,
    ) -> Result<String> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(&format!(
            "{MEMORY_SELECT_WITH_DETAIL} WHERE m.memory_id=$1 FOR UPDATE OF m"
        ))
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StoreError::NotFound("memory".into()))?;
        let old = memory_from_row(&row)?;
        if old.status == Status::Forgotten {
            return Err(StoreError::Conflict(
                "forgotten memories cannot be rewritten".into(),
            ));
        }
        if old.status == Status::Superseded {
            return Err(StoreError::Conflict("rewrite the current version".into()));
        }
        let context = WorkContext {
            project_id: old.project_id.clone(),
            repository: old.context.repo.clone(),
            branch: old.context.branch.clone(),
            worktree: old.context.worktree.clone(),
            commit: old.context.commit.clone(),
            ..WorkContext::default()
        };
        let input = SaveInput {
            kind: old.kind,
            title,
            summary,
            reason,
            detail: Some(old.detail),
            fields: Some(fields.unwrap_or_else(|| old.fields.to_json())),
            evidence: old.evidence.clone(),
            supersedes: Some(id.to_owned()),
            contradicts: None,
            scope: Some(old.scope),
            aliases: Vec::new(),
        };
        let mut prepared = prepare_save(
            input,
            &context,
            old.project_id.is_some(),
            old.origin,
            now_seconds(),
        )?;
        if let Some(hash) = old.chat_id_hash {
            prepared.tag_chat(hash, old.rank == Rank::Candidate);
        }
        if prepared.identity_hash == old.identity_hash {
            return Err(StoreError::Conflict(
                "nothing to rewrite: title, summary and fields are unchanged".into(),
            ));
        }
        let saved = insert_memory(
            &mut tx,
            &old.owner,
            old.project_id.as_deref(),
            &prepared,
            None,
            None,
            false,
        )
        .await?;
        if saved.deduplicated {
            return Err(StoreError::Conflict(
                "rewrite matches an existing memory".into(),
            ));
        }
        sqlx::query("UPDATE memories SET edited_by=$2 WHERE memory_id=$1")
            .bind(&saved.id)
            .bind(admin)
            .execute(&mut *tx)
            .await?;
        crate::account::audit_tx(
            &mut tx,
            admin,
            None,
            "memory_rewritten_by_admin",
            Some(id),
            serde_json::json!({"old_id":id,"new_id":saved.id}),
        )
        .await?;
        tx.commit().await?;
        Ok(saved.id)
    }
}

#[cfg(test)]
mod tests;
