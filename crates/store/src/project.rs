use crate::{Result, Store, StoreError, epoch};
use cairn_core::id::new_id;
use cairn_core::project::{Project, default_project_name, validate_project_text};
use serde::Serialize;
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};
use std::collections::HashMap;

const PROJECT_COLUMNS: &str = "project_id, owner_id, repository, name, default_branch,
    EXTRACT(EPOCH FROM created_at)::bigint AS created_at,
    EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at";

fn project_from_row(row: &PgRow) -> Result<Project> {
    Ok(Project {
        project_id: row.try_get("project_id")?,
        owner_id: row.try_get("owner_id")?,
        repository: row.try_get("repository")?,
        name: row.try_get("name")?,
        default_branch: row.try_get("default_branch")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// The latest handoff the reader may see for a branch: where work stopped.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BranchHandoff {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub fields: serde_json::Value,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub updated_at: i64,
    pub device_name: Option<String>,
}

/// Derived from `memories.repo_branch`; there is no branch table.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BranchSummary {
    pub branch: String,
    /// Project memories saved on the branch (a handoff is not counted).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub memories: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub last_updated: i64,
    pub handoff: Option<BranchHandoff>,
}

/// What the project list shows next to each project.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProjectCounts {
    /// Live project memories (active or stale), including candidates.
    pub memories: i64,
    /// Of `memories`, how many are still rank `candidate`.
    pub candidates: i64,
    pub stale: i64,
    pub members: i64,
    /// The latest memory change or session in the project.
    pub last_activity_at: Option<i64>,
}

/// A pair of live project memories joined by a `contradicts` link.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Conflict {
    pub memory_id: String,
    pub contradicts_id: String,
}

/// Report of a recent session of a project.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionReport {
    pub session_id: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub started_at: i64,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub ended_at: Option<i64>,
    pub branch: Option<String>,
    pub automated: bool,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub distilled_at: Option<i64>,
    pub notes_written: Option<i32>,
    pub digest_written: Option<bool>,
    pub handoff_outcome: Option<String>,
    pub why_none: Option<Vec<String>>,
    pub files_changed: Option<i32>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub captures: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub agent_saves: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Member {
    pub person_id: String,
    pub email: String,
    pub is_owner: bool,
    pub joined_at: i64,
    /// The member's latest session or project memory in this project.
    pub last_active_at: Option<i64>,
}

/// Idempotent. False when the project does not exist.
pub(crate) async fn join_member(
    tx: &mut PgConnection,
    person_id: &str,
    project_id: &str,
) -> Result<(bool, bool)> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM projects WHERE project_id = $1)")
            .bind(project_id)
            .fetch_one(&mut *tx)
            .await?;
    let inserted = if exists {
        sqlx::query("INSERT INTO project_members (project_id, person_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(project_id)
            .bind(person_id)
            .execute(tx)
            .await?
            .rows_affected() == 1
    } else {
        false
    };
    Ok((exists, inserted))
}

impl Store {
    /// `cairn init` without a project id: reuse the person's remote project, or create one
    /// (with the owner as a member). Local folder names always create a new project.
    /// The flag is true when it is new.
    pub async fn init_project(
        &self,
        person_id: &str,
        repository: &str,
        name: Option<&str>,
    ) -> Result<(Project, bool)> {
        let repository = repository.trim();
        validate_project_text(repository, "repository")?;
        let name = name.map(str::trim).filter(|n| !n.is_empty());
        if let Some(name) = name {
            validate_project_text(name, "project name")?;
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('cairn-project-init:' || $1 || ':' || $2, 0))")
            .bind(person_id)
            .bind(repository)
            .execute(&mut *tx)
            .await?;
        let existing = sqlx::query(&format!(
            "SELECT {PROJECT_COLUMNS} FROM projects WHERE owner_id = $1 AND repository = $2
             AND repository NOT LIKE 'local:%'
             ORDER BY created_at, project_id LIMIT 1"
        ))
        .bind(person_id)
        .bind(repository)
        .fetch_optional(&mut *tx)
        .await?;
        let (project, created) = if let Some(row) = existing {
            (project_from_row(&row)?, false)
        } else {
            let default_name = default_project_name(repository);
            let row = sqlx::query(&format!(
                "INSERT INTO projects (project_id, owner_id, repository, name) VALUES ($1, $2, $3, $4)
                 RETURNING {PROJECT_COLUMNS}"
            ))
            .bind(new_id("proj"))
            .bind(person_id)
            .bind(repository)
            .bind(name.unwrap_or(&default_name))
            .fetch_one(&mut *tx)
            .await?;
            let project = project_from_row(&row)?;
            join_member(&mut tx, person_id, &project.project_id).await?;
            (project, true)
        };
        if name.is_some_and(|n| n != project.name) {
            return Err(StoreError::Validation(
                "project name differs from the server; rename it in the web console".into(),
            ));
        }
        tx.commit().await?;
        Ok((project, created))
    }

    /// A committed project id the server does not know (a wiped or rebuilt server): creates the
    /// project with that same id, owned by the caller, so the committed file keeps working for
    /// everyone. An id another caller restored first is simply joined. True when this call created
    /// it. Without `confirmed` it is a conflict: the first caller owns the id.
    pub async fn restore_project(
        &self,
        person_id: &str,
        project_id: &str,
        repository: &str,
        name: Option<&str>,
        confirmed: bool,
    ) -> Result<(Project, bool)> {
        let repository = repository.trim();
        validate_project_text(repository, "repository")?;
        let name = name.map(str::trim).filter(|n| !n.is_empty());
        if let Some(name) = name {
            validate_project_text(name, "project name")?;
        }
        let shaped = project_id.strip_prefix("proj_").is_some_and(|rest| {
            !rest.is_empty()
                && project_id.len() <= 64
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        });
        if !shaped {
            return Err(StoreError::Validation("invalid project id".into()));
        }
        // Whoever registers an unknown id first owns it, so the caller must say they know that.
        if !confirmed {
            return Err(StoreError::Conflict(format!(
                "this server does not know project {project_id} (wiped or new server?). Registering it again makes you its owner; if that is intended, run `cairn init --yes`"
            )));
        }
        let mut tx = self.pool.begin().await?;
        let default_name = default_project_name(repository);
        let created = sqlx::query(
            "INSERT INTO projects (project_id, owner_id, repository, name) VALUES ($1, $2, $3, $4)
             ON CONFLICT (project_id) DO NOTHING",
        )
        .bind(project_id)
        .bind(person_id)
        .bind(repository)
        .bind(name.unwrap_or(&default_name))
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        join_member(&mut tx, person_id, project_id).await?;
        let row = sqlx::query(&format!(
            "SELECT {PROJECT_COLUMNS} FROM projects WHERE project_id = $1"
        ))
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok((project_from_row(&row)?, created))
    }

    /// Membership through the committed project file; the server name is authoritative.
    pub async fn join_project(
        &self,
        person_id: &str,
        project_id: &str,
        name: Option<&str>,
    ) -> Result<Project> {
        self.join_project_from(person_id, project_id, name, None)
            .await
    }

    pub async fn join_project_from(
        &self,
        person_id: &str,
        project_id: &str,
        _name: Option<&str>,
        device_id: Option<&str>,
    ) -> Result<Project> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(&format!(
            "SELECT {PROJECT_COLUMNS} FROM projects WHERE project_id = $1 FOR SHARE"
        ))
        .bind(project_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StoreError::NotFound("project".into()))?;
        let project = project_from_row(&row)?;
        let (_, inserted) = join_member(&mut tx, person_id, project_id).await?;
        if inserted {
            crate::account::audit_tx(&mut tx, person_id, device_id, "project_auto_joined", Some(project_id), serde_json::json!({"project_id":project_id,"person":person_id,"device":device_id,"source":"init"})).await?;
        }
        tx.commit().await?;
        Ok(project)
    }

    pub async fn rename_project(
        &self,
        person_id: &str,
        project_id: &str,
        name: &str,
    ) -> Result<Project> {
        let name = name.trim();
        validate_project_text(name, "project name")?;
        let row = sqlx::query(&format!(
            "UPDATE projects SET name = $3, updated_at = now() WHERE project_id = $1 AND owner_id = $2
             RETURNING {PROJECT_COLUMNS}"
        ))
        .bind(project_id)
        .bind(person_id)
        .bind(name)
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => project_from_row(&row),
            None => Err(StoreError::Forbidden(
                "account does not own the project".into(),
            )),
        }
    }

    /// The owner, or an admin, hands the project to an existing member.
    pub async fn transfer_project(
        &self,
        actor_id: &str,
        actor_is_admin: bool,
        project_id: &str,
        new_owner: &str,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let owner: String =
            sqlx::query_scalar("SELECT owner_id FROM projects WHERE project_id = $1 FOR UPDATE")
                .bind(project_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| StoreError::NotFound("project".into()))?;
        if owner != actor_id && !actor_is_admin {
            return Err(StoreError::Forbidden(
                "account does not own the project".into(),
            ));
        }
        let member: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM project_members WHERE project_id = $1 AND person_id = $2)",
        )
        .bind(project_id)
        .bind(new_owner)
        .fetch_one(&mut *tx)
        .await?;
        if !member {
            return Err(StoreError::Validation(
                "new owner must be a project member".into(),
            ));
        }
        sqlx::query("UPDATE projects SET owner_id = $2, updated_at = now() WHERE project_id = $1")
            .bind(project_id)
            .bind(new_owner)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Projects the person belongs to, newest first.
    pub async fn list_projects(&self, person_id: &str) -> Result<Vec<Project>> {
        let rows = sqlx::query(&format!(
            "SELECT {PROJECT_COLUMNS} FROM projects
             WHERE project_id IN (SELECT project_id FROM project_members WHERE person_id = $1)
             ORDER BY created_at DESC, project_id"
        ))
        .bind(person_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(project_from_row).collect()
    }

    /// The current name, visible only to a project member.
    pub async fn project_name(&self, person_id: &str, project_id: &str) -> Result<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT p.name FROM projects p JOIN project_members m ON m.project_id = p.project_id WHERE p.project_id = $1 AND m.person_id = $2",
        )
        .bind(project_id)
        .bind(person_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn is_member(&self, person_id: &str, project_id: &str) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM project_members WHERE project_id = $1 AND person_id = $2)",
        )
        .bind(project_id)
        .bind(person_id)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn project_members(&self, person_id: &str, project_id: &str) -> Result<Vec<Member>> {
        self.require_member(person_id, project_id).await?;
        let rows = sqlx::query(&format!(
            "SELECT m.person_id, a.email, (p.owner_id = m.person_id) AS is_owner, {} AS joined_at,
                    EXTRACT(EPOCH FROM greatest(
                        (SELECT max(s.started_at) FROM sessions s
                          WHERE s.project_id = m.project_id AND s.person_id = m.person_id),
                        (SELECT max(x.created_at) FROM memories x
                          WHERE x.project_id = m.project_id AND x.owner_id = m.person_id
                            AND x.scope = 'project')))::bigint AS last_active_at
               FROM project_members m
               JOIN accounts a USING (person_id)
               JOIN projects p USING (project_id)
              WHERE m.project_id = $1 ORDER BY m.joined_at, m.person_id",
            epoch!("m.joined_at")
        ))
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(Member {
                    person_id: r.try_get("person_id")?,
                    email: r.try_get("email")?,
                    is_owner: r.try_get("is_owner")?,
                    joined_at: r.try_get("joined_at")?,
                    last_active_at: r.try_get("last_active_at")?,
                })
            })
            .collect()
    }

    /// Distinct branches of the project's live memories visible to the person, each with the
    /// person's latest handoff there. A branch that has only a handoff is listed too.
    pub async fn project_branches(
        &self,
        person_id: &str,
        project_id: &str,
    ) -> Result<Vec<BranchSummary>> {
        self.require_member(person_id, project_id).await?;
        let rows = sqlx::query(&format!(
            "SELECT m.repo_branch AS branch, count(*) FILTER (WHERE m.kind <> 'handoff') AS memories,
                    max({}) AS last_updated
               FROM memories m
              WHERE m.project_id = $1 AND m.repo_branch IS NOT NULL
                AND (m.scope = 'project' OR (m.kind = 'handoff' AND m.owner_id = $2))
                AND m.status <> 'forgotten'
              GROUP BY m.repo_branch ORDER BY max(m.updated_at) DESC, m.repo_branch",
            epoch!("m.updated_at")
        ))
        .bind(project_id)
        .bind(person_id)
        .fetch_all(&self.pool)
        .await?;
        let mut handoffs: HashMap<String, BranchHandoff> = HashMap::new();
        for r in sqlx::query(&format!(
            "SELECT DISTINCT ON (m.repo_branch) m.repo_branch AS branch, m.memory_id, m.title, m.summary, m.fields,
                    {} AS updated_at, CASE WHEN m.owner_id = $1 THEN d.name END AS device_name
               FROM memories m LEFT JOIN devices d ON d.device_id = m.device_id AND d.person_id = m.owner_id
              WHERE m.project_id = $2 AND m.kind = 'handoff' AND m.repo_branch IS NOT NULL AND {}
              ORDER BY m.repo_branch, m.updated_at DESC, m.memory_id DESC",
            epoch!("m.updated_at"),
            crate::memory::visible("m", "$1")
        ))
        .bind(person_id)
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?
        {
            handoffs.insert(
                r.try_get("branch")?,
                BranchHandoff {
                    id: r.try_get("memory_id")?,
                    title: r.try_get("title")?,
                    summary: r.try_get("summary")?,
                    fields: r.try_get("fields")?,
                    updated_at: r.try_get("updated_at")?,
                    device_name: r.try_get("device_name")?,
                },
            );
        }
        rows.iter()
            .map(|r| {
                let branch: String = r.try_get("branch")?;
                Ok(BranchSummary {
                    handoff: handoffs.remove(&branch),
                    branch,
                    memories: r.try_get("memories")?,
                    last_updated: r.try_get("last_updated")?,
                })
            })
            .collect()
    }

    /// Counts for the projects `project_ids` (which the caller already knows the person is in).
    pub async fn project_counts(
        &self,
        project_ids: &[String],
    ) -> Result<HashMap<String, ProjectCounts>> {
        let rows = sqlx::query(
            "SELECT p.project_id,
                    (SELECT count(*) FROM memories m WHERE m.project_id = p.project_id AND m.scope = 'project'
                        AND m.status IN ('active','stale')) AS memories,
                    (SELECT count(*) FROM memories m WHERE m.project_id = p.project_id AND m.scope = 'project'
                        AND m.rank = 'candidate'
                        AND m.status IN ('active','stale')) AS candidates,
                    (SELECT count(*) FROM memories m WHERE m.project_id = p.project_id AND m.scope = 'project'
                        AND m.status = 'stale') AS stale,
                    (SELECT count(*) FROM project_members pm WHERE pm.project_id = p.project_id) AS members,
                    EXTRACT(EPOCH FROM greatest(
                        (SELECT max(m.updated_at) FROM memories m
                          WHERE m.project_id = p.project_id AND m.scope = 'project' AND m.status <> 'forgotten'),
                        (SELECT max(s.started_at) FROM sessions s WHERE s.project_id = p.project_id)))::bigint AS last_activity_at
               FROM projects p WHERE p.project_id = ANY($1)",
        )
        .bind(project_ids)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok((
                    r.try_get("project_id")?,
                    ProjectCounts {
                        memories: r.try_get("memories")?,
                        candidates: r.try_get("candidates")?,
                        stale: r.try_get("stale")?,
                        members: r.try_get("members")?,
                        last_activity_at: r.try_get("last_activity_at")?,
                    },
                ))
            })
            .collect()
    }

    /// `contradicts` links between live (active or stale) project memories.
    pub async fn project_conflicts(
        &self,
        person_id: &str,
        project_id: &str,
    ) -> Result<Vec<Conflict>> {
        self.require_member(person_id, project_id).await?;
        let rows = sqlx::query(
            "SELECT l.source_id, l.target_id
               FROM links l
               JOIN memories s ON s.memory_id = l.source_id
               JOIN memories t ON t.memory_id = l.target_id
              WHERE l.relation = 'contradicts'
                AND s.project_id = $1 AND t.project_id = $1
                AND s.scope = 'project' AND t.scope = 'project'
                AND s.status IN ('active','stale') AND t.status IN ('active','stale')
              ORDER BY l.created_at, l.source_id",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(Conflict {
                    memory_id: r.try_get("source_id")?,
                    contradicts_id: r.try_get("target_id")?,
                })
            })
            .collect()
    }

    /// Recent sessions of the project (newest first, up to 50) within `days`.
    pub async fn project_sessions(
        &self,
        person_id: &str,
        project_id: &str,
        days: i32,
    ) -> Result<Vec<SessionReport>> {
        self.require_member(person_id, project_id).await?;
        let rows = sqlx::query(
            "SELECT s.session_id,
                    EXTRACT(EPOCH FROM s.started_at)::bigint AS started_at,
                    EXTRACT(EPOCH FROM s.ended_at)::bigint AS ended_at,
                    (s.context->>'branch') AS branch,
                    (COALESCE(s.context->>'origin', '') = 'automation') AS automated,
                    EXTRACT(EPOCH FROM s.distilled_at)::bigint AS distilled_at,
                    s.notes_written,
                    s.digest_written,
                    s.handoff_outcome,
                    s.why_none,
                    s.files_changed,
                    (SELECT count(*) FROM observations o
                      WHERE o.person_id = s.person_id AND o.session_id = s.session_id
                        AND o.expires_at > now()) AS captures,
                    (SELECT count(*) FROM memories m
                      WHERE m.owner_id = s.person_id
                        AND m.project_id = s.project_id
                        AND m.origin = 'agent'
                        AND m.created_at >= s.started_at
                        AND m.created_at <= COALESCE(
                            s.ended_at,
                            (SELECT max(o.captured_at) FROM observations o WHERE o.person_id = s.person_id AND o.session_id = s.session_id),
                            s.started_at
                        )) AS agent_saves
               FROM sessions s
              WHERE s.project_id = $1
                AND s.started_at >= now() - make_interval(days => $2)
              ORDER BY s.started_at DESC, s.session_id DESC
              LIMIT 50",
        )
        .bind(project_id)
        .bind(days)
        .fetch_all(&self.pool)
        .await?;

        rows.iter()
            .map(|r| {
                Ok(SessionReport {
                    session_id: r.try_get("session_id")?,
                    started_at: r.try_get("started_at")?,
                    ended_at: r.try_get("ended_at")?,
                    branch: r.try_get("branch")?,
                    automated: r.try_get("automated")?,
                    distilled_at: r.try_get("distilled_at")?,
                    notes_written: r.try_get("notes_written")?,
                    digest_written: r.try_get("digest_written")?,
                    handoff_outcome: r.try_get("handoff_outcome")?,
                    why_none: r.try_get("why_none")?,
                    files_changed: r.try_get("files_changed")?,
                    captures: r.try_get("captures")?,
                    agent_saves: r.try_get("agent_saves")?,
                })
            })
            .collect()
    }

    async fn require_member(&self, person_id: &str, project_id: &str) -> Result<()> {
        if self.is_member(person_id, project_id).await? {
            Ok(())
        } else {
            Err(StoreError::NotFound("project".into()))
        }
    }
}
