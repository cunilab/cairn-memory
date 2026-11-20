use crate::{Result, Store, StoreError, epoch};
use cairn_core::id::new_id;
use cairn_core::project::{Project, default_project_name, validate_project_text};
use serde::Serialize;
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};

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

/// Derived from `memories.repo_branch`; there is no branch table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BranchSummary {
    pub branch: String,
    pub memories: i64,
    pub last_updated: i64,
}

/// A pair of live project memories joined by a `contradicts` link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Conflict {
    pub memory_id: String,
    pub contradicts_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Member {
    pub person_id: String,
    pub email: String,
    pub is_owner: bool,
    pub joined_at: i64,
}

/// Idempotent. False when the project does not exist.
pub(crate) async fn join_member(
    tx: &mut PgConnection,
    person_id: &str,
    project_id: &str,
) -> Result<bool> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM projects WHERE project_id = $1)")
            .bind(project_id)
            .fetch_one(&mut *tx)
            .await?;
    if exists {
        sqlx::query("INSERT INTO project_members (project_id, person_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(project_id)
            .bind(person_id)
            .execute(tx)
            .await?;
    }
    Ok(exists)
}

impl Store {
    /// `cairn init`: the person's project for the repository, created (with the owner as a member)
    /// when there is none. The flag is true when it is new.
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
                "project name differs from the server; use `cairn init --rename <name>` as the owner".into(),
            ));
        }
        tx.commit().await?;
        Ok((project, created))
    }

    /// Membership through the committed project file. Name, when given, must match the server's.
    pub async fn join_project(
        &self,
        person_id: &str,
        project_id: &str,
        name: Option<&str>,
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
        if name.is_some_and(|n| n.trim() != project.name) {
            return Err(StoreError::Validation(
                "project name differs from the server; use `cairn init --rename <name>` as the owner".into(),
            ));
        }
        join_member(&mut tx, person_id, project_id).await?;
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
            "SELECT m.person_id, a.email, (p.owner_id = m.person_id) AS is_owner, {} AS joined_at
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
                })
            })
            .collect()
    }

    /// Distinct branches of the project's live memories visible to the person.
    pub async fn project_branches(
        &self,
        person_id: &str,
        project_id: &str,
    ) -> Result<Vec<BranchSummary>> {
        self.require_member(person_id, project_id).await?;
        let rows = sqlx::query(&format!(
            "SELECT m.repo_branch AS branch, count(*) AS memories, max({}) AS last_updated
               FROM memories m
              WHERE m.project_id = $1 AND m.scope = 'project' AND m.repo_branch IS NOT NULL
                AND m.status <> 'forgotten' AND (m.expires_at IS NULL OR m.expires_at > now())
              GROUP BY m.repo_branch ORDER BY max(m.updated_at) DESC, m.repo_branch",
            epoch!("m.updated_at")
        ))
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(BranchSummary {
                    branch: r.try_get("branch")?,
                    memories: r.try_get("memories")?,
                    last_updated: r.try_get("last_updated")?,
                })
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
                AND (s.expires_at IS NULL OR s.expires_at > now())
                AND (t.expires_at IS NULL OR t.expires_at > now())
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

    async fn require_member(&self, person_id: &str, project_id: &str) -> Result<()> {
        if self.is_member(person_id, project_id).await? {
            Ok(())
        } else {
            Err(StoreError::NotFound("project".into()))
        }
    }
}
