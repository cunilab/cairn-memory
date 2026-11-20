use crate::jobs::enqueue_tx;
use crate::project::join_member;
use crate::{Result, Store, StoreError};
use cairn_core::capture::WorkContext;
use cairn_core::id::{new_id, now_seconds};
use cairn_core::memory::{
    Context, Entity, EntityKind, Fields, Kind, Link, Memory, Relation, SaveInput, Scope, Status,
    Usage,
};
use cairn_core::process::jobs::JobKind;
use cairn_core::process::{NewMemory, Origin, prepare_save};
use cairn_core::retrieve::rank::status_after_wrong;
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};
use std::collections::{HashMap, HashSet};

/// The visibility rule, reused by every read: not forgotten, not expired, and owned, global, or
/// in a project the reader belongs to. `alias` is the memories table alias, `me` a bind like `$1`.
pub(crate) fn visible(alias: &str, me: &str) -> String {
    format!(
        "({a}.status <> 'forgotten' AND ({a}.expires_at IS NULL OR {a}.expires_at > now())
          AND ({a}.owner_id = {me} OR {a}.scope = 'global'
               OR ({a}.scope = 'project' AND {a}.project_id IN
                    (SELECT project_id FROM project_members WHERE person_id = {me}))))",
        a = alias
    )
}

pub(crate) const MEMORY_SELECT: &str =
    "SELECT m.memory_id, m.owner_id, m.project_id, m.scope, m.kind,
    m.title, m.summary, m.reason, m.fields, m.evidence, m.evidence_count, m.evidence_hash,
    m.identity_hash, m.repo, m.repo_branch, m.repo_worktree, m.repo_commit, m.status,
    EXTRACT(EPOCH FROM m.expires_at)::bigint AS expires_at,
    EXTRACT(EPOCH FROM m.created_at)::bigint AS created_at,
    EXTRACT(EPOCH FROM m.updated_at)::bigint AS updated_at,
    to_char(m.updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS cursor_ts,
    COALESCE(u.fetched, 0) AS fetched, COALESCE(u.helpful, 0) AS helpful,
    COALESCE(u.wrong, 0) AS wrong, EXTRACT(EPOCH FROM u.last_used_at)::bigint AS last_used_at
    FROM memories m LEFT JOIN usage u ON u.memory_id = m.memory_id";

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
        expires_at: row.try_get("expires_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        evidence_hash: row.try_get("evidence_hash")?,
        identity_hash: row.try_get("identity_hash")?,
        usage: Usage {
            fetched: row.try_get::<i32, _>("fetched")?.max(0) as u32,
            helpful: row.try_get::<i32, _>("helpful")?.max(0) as u32,
            wrong: row.try_get::<i32, _>("wrong")?.max(0) as u32,
            last_used_at: row.try_get("last_used_at")?,
        },
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    pub id: String,
    pub status: Status,
    /// True when an identical memory already existed; nothing new was written.
    pub deduplicated: bool,
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
    /// 1..=500, default 100.
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Page {
    pub memories: Vec<Memory>,
    pub next_cursor: Option<String>,
}

impl Store {
    /// An agent's save. Project membership comes from the work context's project id.
    pub async fn save(
        &self,
        person_id: &str,
        input: SaveInput,
        work: &WorkContext,
    ) -> Result<Saved> {
        self.save_as(person_id, input, work, Origin::Agent).await
    }

    pub(crate) async fn save_as(
        &self,
        person_id: &str,
        input: SaveInput,
        work: &WorkContext,
        origin: Origin,
    ) -> Result<Saved> {
        let project_id = self.context_project(person_id, work).await?;
        let new = prepare_save(input, work, project_id.is_some(), origin, now_seconds())?;
        let mut tx = self.pool.begin().await?;
        let saved = insert_memory(&mut tx, person_id, project_id.as_deref(), &new, None).await?;
        tx.commit().await?;
        Ok(saved)
    }

    /// The request's project when the person is (or by the committed file becomes) a member.
    pub(crate) async fn context_project(
        &self,
        person_id: &str,
        work: &WorkContext,
    ) -> Result<Option<String>> {
        let Some(project_id) = work
            .project_id
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
        else {
            return Ok(None);
        };
        let mut tx = self.pool.begin().await?;
        let exists = join_member(&mut tx, person_id, project_id).await?;
        tx.commit().await?;
        Ok(exists.then(|| project_id.to_owned()))
    }

    pub async fn get(&self, person_id: &str, memory_id: &str) -> Result<Memory> {
        let row = sqlx::query(&format!(
            "{MEMORY_SELECT} WHERE m.memory_id = $2 AND {}",
            visible("m", "$1")
        ))
        .bind(person_id)
        .bind(memory_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("memory".into()))?;
        let mut memories = vec![memory_from_row(&row)?];
        self.attach(person_id, &mut memories).await?;
        Ok(memories.remove(0))
    }

    /// Counts an explicit fetch by an agent (`get` or search). Internal usage; never shown.
    pub async fn mark_fetched(&self, memory_ids: &[String]) -> Result<()> {
        sqlx::query(
            "INSERT INTO usage (memory_id, fetched, last_used_at)
             SELECT id, 1, now() FROM unnest($1::text[]) AS id WHERE EXISTS (SELECT 1 FROM memories WHERE memory_id = id)
             ON CONFLICT (memory_id) DO UPDATE SET fetched = usage.fetched + 1, last_used_at = now()",
        )
        .bind(memory_ids)
        .execute(&self.pool)
        .await?;
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
        let rows = sqlx::query(&format!(
            "{MEMORY_SELECT}
             WHERE {} AND ($2::text IS NULL OR m.project_id = $2)
               AND ($3::text IS NULL OR m.scope = $3) AND ($4::text IS NULL OR m.kind = $4)
               AND (cardinality($5::text[]) = 0 OR m.status = ANY($5))
               AND ($6::text IS NULL OR (m.updated_at, m.memory_id) < ($6::timestamptz, $7::text))
             ORDER BY m.updated_at DESC, m.memory_id DESC LIMIT $8",
            visible("m", "$1")
        ))
        .bind(person_id)
        .bind(&filter.project_id)
        .bind(filter.scope.map(Scope::as_str))
        .bind(filter.kind.map(Kind::as_str))
        .bind(&statuses)
        .bind(cursor_ts)
        .bind(cursor_id)
        .bind(limit + 1)
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
            .map(memory_from_row)
            .collect::<Result<Vec<_>>>()?;
        self.attach(person_id, &mut memories).await?;
        Ok(Page {
            memories,
            next_cursor,
        })
    }

    /// Fills `entities` and `links` (outgoing and incoming). A link whose other end the reader
    /// cannot see is left out, so links never reveal a hidden memory.
    pub(crate) async fn attach(&self, person_id: &str, memories: &mut [Memory]) -> Result<()> {
        if memories.is_empty() {
            return Ok(());
        }
        let ids: Vec<String> = memories.iter().map(|m| m.id.clone()).collect();
        let mut entities: HashMap<String, Vec<Entity>> = HashMap::new();
        for row in sqlx::query("SELECT memory_id, kind, name FROM entities WHERE memory_id = ANY($1) ORDER BY kind, name")
            .bind(&ids)
            .fetch_all(&self.pool)
            .await?
        {
            let kind: EntityKind = parse(&row, "kind")?;
            entities.entry(row.try_get("memory_id")?).or_default().push(Entity { kind, name: row.try_get("name")? });
        }
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
            visible("m", "$1")
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

    /// `helpful` or `wrong`. Feeds ranking and, for `wrong`, the stale rule.
    pub async fn feedback(&self, person_id: &str, memory_id: &str, vote: Vote) -> Result<Status> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(&format!(
            "SELECT m.owner_id, m.status FROM memories m WHERE m.memory_id = $2 AND {} FOR UPDATE OF m",
            visible("m", "$1")
        ))
        .bind(person_id)
        .bind(memory_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StoreError::NotFound("memory".into()))?;
        let owner: String = row.try_get("owner_id")?;
        let status: Status = parse(&row, "status")?;
        let counts = sqlx::query(
            "INSERT INTO usage (memory_id, helpful, wrong) VALUES ($1, $2, $3)
             ON CONFLICT (memory_id) DO UPDATE
                SET helpful = usage.helpful + $2, wrong = usage.wrong + $3
             RETURNING wrong",
        )
        .bind(memory_id)
        .bind(i32::from(vote == Vote::Helpful))
        .bind(i32::from(vote == Vote::Wrong))
        .fetch_one(&mut *tx)
        .await?;
        let wrong = counts.try_get::<i32, _>("wrong")?.max(0) as u32;
        let new_status = if vote == Vote::Wrong {
            status_after_wrong(status, wrong, owner == person_id)
        } else {
            status
        };
        if new_status != status {
            sqlx::query("UPDATE memories SET status = $2, updated_at = now() WHERE memory_id = $1")
                .bind(memory_id)
                .bind(new_status.as_str())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(new_status)
    }

    /// Owner only. The row stays as `forgotten` with its text blanked and `identity_hash` kept, so
    /// saving the same thing again is a 409. Embedding, entity and link rows are deleted, and a
    /// memory this one superseded comes back unless another live memory also supersedes it.
    pub async fn forget(&self, person_id: &str, memory_id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM memories WHERE memory_id = $1 AND owner_id = $2 FOR UPDATE",
        )
        .bind(memory_id)
        .bind(person_id)
        .fetch_optional(&mut *tx)
        .await?;
        match status.as_deref() {
            None => return Err(StoreError::NotFound("memory".into())),
            Some("forgotten") => return Ok(()),
            _ => {}
        }
        let restored: Vec<String> = sqlx::query_scalar(
            "SELECT target_id FROM links WHERE source_id = $1 AND relation = 'supersedes'",
        )
        .bind(memory_id)
        .fetch_all(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE memories SET status = 'forgotten', title = '', summary = '', reason = '', fields = '{}',
                    evidence = '{}', evidence_count = 0, repo = NULL, repo_branch = NULL,
                    repo_worktree = NULL, repo_commit = NULL, updated_at = now()
              WHERE memory_id = $1",
        )
        .bind(memory_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM memory_embeddings WHERE memory_id = $1")
            .bind(memory_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM entities WHERE memory_id = $1")
            .bind(memory_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM links WHERE source_id = $1 OR target_id = $1")
            .bind(memory_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM jobs WHERE kind IN ('embed','consolidate') AND job_key = $1 AND status = 'pending'")
            .bind(memory_id)
            .execute(&mut *tx)
            .await?;
        for target in restored {
            restore_if_unsuperseded(&mut tx, &target).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// A graph edge written by an agent. `supersedes` also hides the target (same rules as save).
    pub async fn add_link(
        &self,
        person_id: &str,
        source: &str,
        target: &str,
        relation: Relation,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(&format!(
            "SELECT m.scope, m.project_id FROM memories m WHERE m.memory_id = $2 AND m.owner_id = $1 AND {}",
            visible("m", "$1")
        ))
        .bind(person_id)
        .bind(source)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StoreError::NotFound("memory".into()))?;
        let scope: Scope = parse(&row, "scope")?;
        let project_id: Option<String> = row.try_get("project_id")?;
        link_tx(
            &mut tx,
            person_id,
            source,
            scope,
            project_id.as_deref(),
            target,
            relation,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn remove_link(
        &self,
        person_id: &str,
        source: &str,
        target: &str,
        relation: Relation,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let deleted = sqlx::query(
            "DELETE FROM links l USING memories m
              WHERE l.source_id = $2 AND l.target_id = $3 AND l.relation = $4
                AND m.memory_id = l.source_id AND m.owner_id = $1",
        )
        .bind(person_id)
        .bind(source)
        .bind(target)
        .bind(relation.as_str())
        .execute(&mut *tx)
        .await?;
        if deleted.rows_affected() == 0 {
            return Err(StoreError::NotFound("link".into()));
        }
        if relation == Relation::Supersedes {
            restore_if_unsuperseded(&mut tx, target).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

/// Back to `active` when no remaining live memory supersedes it.
async fn restore_if_unsuperseded(tx: &mut PgConnection, memory_id: &str) -> Result<()> {
    sqlx::query(
        "UPDATE memories SET status = 'active', updated_at = now()
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
        "SELECT m.owner_id, m.scope, m.project_id FROM memories m WHERE m.memory_id = $2 AND {}",
        visible("m", "$1")
    ))
    .bind(person_id)
    .bind(target)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| StoreError::NotFound(format!("memory {target}")))?;
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
        sqlx::query("UPDATE memories SET status = 'superseded', updated_at = now() WHERE memory_id = $1 AND status IN ('active','stale')")
            .bind(target)
            .execute(tx)
            .await?;
    }
    Ok(())
}

/// Inserts a derived memory with its entities, usage row, agent links and embed job. An identical
/// memory (same owner, scope, project and identity hash) returns as a duplicate, or is revived when
/// it had expired; a forgotten one is a 409. `fixed_id` is for the deterministic handoff id.
pub(crate) async fn insert_memory(
    tx: &mut PgConnection,
    person_id: &str,
    project_id: Option<&str>,
    new: &NewMemory,
    fixed_id: Option<&str>,
) -> Result<Saved> {
    let project_id = if new.scope == Scope::Global {
        None
    } else {
        project_id
    };
    let id = fixed_id.map_or_else(|| new_id("mem"), str::to_owned);
    let inserted: Option<String> = sqlx::query_scalar(
        "INSERT INTO memories (memory_id, owner_id, project_id, scope, kind, title, summary, reason, fields,
                               evidence, evidence_count, evidence_hash, identity_hash,
                               repo, repo_branch, repo_worktree, repo_commit, expires_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17, to_timestamp($18))
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
    .fetch_optional(&mut *tx)
    .await?;
    let Some(id) = inserted else {
        let existing = sqlx::query(
            "SELECT memory_id, status, coalesce(expires_at <= now(), false) AS expired FROM memories
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
        // Saving again after expiry gives the same memory a new life; until the sweep deletes the
        // row it would otherwise stay hidden while the save reported success.
        let revived: bool = existing.try_get("expired")?;
        if revived {
            sqlx::query(
                "UPDATE memories SET expires_at = to_timestamp($2), updated_at = now() WHERE memory_id = $1",
            )
            .bind(&id)
            .bind(new.expires_at.map(|t| t as f64))
            .execute(&mut *tx)
            .await?;
            enqueue_tx(tx, JobKind::Embed, &id).await?;
        }
        save_links(tx, person_id, &id, project_id, new).await?;
        return Ok(Saved {
            id,
            status,
            deduplicated: !revived,
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
    Ok(Saved {
        id,
        status: Status::Active,
        deduplicated: false,
    })
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
        if let Some(target) = target {
            link_tx(tx, person_id, id, new.scope, project_id, target, relation).await?;
        }
    }
    Ok(())
}
