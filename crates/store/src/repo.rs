//! Git facts reported by the hook (apps/docs/content/technical/hooks/index.md#events): which files and symbols a project's memories
//! cite still exist on the default branch, and which branches were merged into it.

use crate::{Result, Store, StoreError};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::{HashMap, HashSet};

pub const CHECK_FILES_MAX: i64 = 200;
pub const CHECK_SYMBOLS_MAX: i64 = 20;
pub const MERGED_BRANCHES_MAX: usize = 50;

/// What the brief asks the hook to look up (`POST /session/brief` reply, `check`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BriefCheck {
    pub project_id: String,
    pub branch: String,
    pub files: Vec<String>,
    pub symbols: Vec<String>,
}

/// The hook's answer (`POST /session/repo-state`). Lists are what it looked up, not all it knows.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoState {
    pub project_id: String,
    pub branch: String,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub files_missing: Vec<String>,
    #[serde(default)]
    pub files_present: Vec<String>,
    #[serde(default)]
    pub symbols_missing: Vec<String>,
    #[serde(default)]
    pub symbols_present: Vec<String>,
    #[serde(default)]
    pub merged_branches: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RepoStateReport {
    pub stale: usize,
    pub revived: usize,
    pub promoted: usize,
}

/// The project's default branch, `main` or `master` when none is set.
fn is_default(default_branch: Option<&str>, branch: &str) -> bool {
    match default_branch {
        Some(default) => default == branch,
        None => branch == "main" || branch == "master",
    }
}

/// The branch names a memory may have been made on to count as "on the default branch".
fn default_names(default_branch: Option<&str>) -> Vec<String> {
    match default_branch {
        Some(default) => vec![default.to_owned()],
        None => vec!["main".to_owned(), "master".to_owned()],
    }
}

/// Only a repo-relative file can be judged gone: an absolute, home or parent-relative name points
/// outside the checkout, so its absence says nothing. SQL over entity alias `e`.
const IN_REPO: &str = "(e.kind <> 'file' OR (e.name !~ '^[/~]' AND e.name !~ '(^|/)\\.\\.(/|$)' AND position(':' in e.name) = 0))";

fn set(items: &[String]) -> HashSet<&str> {
    items.iter().map(String::as_str).collect()
}

/// A memory's cited files and symbols, and whether the report decides it is gone or back.
struct Cited {
    status: String,
    files: HashSet<String>,
    symbols: HashSet<String>,
}

impl Store {
    /// `Some` only for a member on the project's default branch: the files and symbols (least
    /// recently checked first) of its project memories that are live, or stale because missing.
    pub async fn brief_check(
        &self,
        person_id: &str,
        project_id: &str,
        branch: &str,
    ) -> Result<Option<BriefCheck>> {
        let default_branch: Option<Option<String>> = sqlx::query_scalar(
            "SELECT default_branch FROM projects WHERE project_id = $1 AND archived_at IS NULL",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(default_branch) = default_branch else {
            return Ok(None);
        };
        if !is_default(default_branch.as_deref(), branch)
            || !self.is_member(person_id, project_id).await?
        {
            return Ok(None);
        }
        let active = crate::memory::active_project_memory("m");
        let on_default = default_names(default_branch.as_deref());
        let mut lists = Vec::new();
        for (kind, limit) in [("file", CHECK_FILES_MAX), ("symbol", CHECK_SYMBOLS_MAX)] {
            // Never checked first, then the oldest check, then by name.
            let names: Vec<String> = sqlx::query_scalar(&format!(
                "SELECT e.name FROM entities e JOIN memories m ON m.memory_id = e.memory_id
                  WHERE m.project_id = $1 AND m.scope = 'project' AND e.kind = $2 AND {IN_REPO} AND {active}
                    AND (m.repo_branch IS NULL OR m.repo_branch = ANY($3))
                    AND (m.status = 'active' OR (m.status = 'stale' AND m.stale_cause = 'missing'))
                  GROUP BY e.name
                  ORDER BY bool_or(m.checked_at IS NULL) DESC, min(m.checked_at), e.name
                  LIMIT {limit}"
            ))
            .bind(project_id)
            .bind(kind)
            .bind(&on_default)
            .fetch_all(&self.pool)
            .await?;
            lists.push(names);
        }
        let symbols = lists.pop().unwrap_or_default();
        let files = lists.pop().unwrap_or_default();
        Ok(Some(BriefCheck {
            project_id: project_id.to_owned(),
            branch: branch.to_owned(),
            files,
            symbols,
        }))
    }

    /// Applies the hook's report in one transaction: a memory whose cited files (and symbols) are
    /// all gone becomes `stale` (`missing`); one that was stale for that reason and has a cited
    /// file or symbol back becomes `active`; then branches merged into the default branch move
    /// the reporter's own memories to it and expire the reporter's own handoffs (never another
    /// member's branches or handoffs). Rejected for a non-member or a branch that
    /// is not the project's default.
    pub async fn repo_state(&self, person_id: &str, state: &RepoState) -> Result<RepoStateReport> {
        let default_branch: Option<Option<String>> =
            sqlx::query_scalar("SELECT default_branch FROM projects WHERE project_id = $1")
                .bind(&state.project_id)
                .fetch_optional(&self.pool)
                .await?;
        let Some(default_branch) = default_branch else {
            return Err(StoreError::Validation(
                "not a member of this project".into(),
            ));
        };
        if !self.is_member(person_id, &state.project_id).await? {
            return Err(StoreError::Validation(
                "not a member of this project".into(),
            ));
        }
        if !is_default(default_branch.as_deref(), &state.branch) {
            return Err(StoreError::Validation(
                "repo state is read on the project's default branch".into(),
            ));
        }
        let merged: Vec<String> = state
            .merged_branches
            .iter()
            .filter(|b| !b.is_empty() && **b != state.branch)
            .take(MERGED_BRANCHES_MAX)
            .cloned()
            .collect();
        let (files_missing, files_present) = (set(&state.files_missing), set(&state.files_present));
        let (symbols_missing, symbols_present) =
            (set(&state.symbols_missing), set(&state.symbols_present));

        let on_default = default_names(default_branch.as_deref());
        let reported: Vec<&str> = [
            &state.files_missing,
            &state.files_present,
            &state.symbols_missing,
            &state.symbols_present,
        ]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect();
        let mut tx = self.pool.begin().await?;
        crate::project::lock_active_project(&mut tx, &state.project_id).await?;
        // Only memories that cite a reported name are read and locked, with all their entities.
        let rows = sqlx::query(&format!(
            "SELECT m.memory_id, m.status, e.kind, e.name
               FROM memories m JOIN entities e ON e.memory_id = m.memory_id
              WHERE m.project_id = $1 AND m.scope = 'project' AND e.kind IN ('file','symbol') AND {IN_REPO}
                AND (m.repo_branch IS NULL OR m.repo_branch = ANY($3))
                AND (m.status = 'active' OR (m.status = 'stale' AND m.stale_cause = 'missing'))
                AND m.memory_id IN (SELECT r.memory_id FROM entities r WHERE r.name = ANY($2)
                                     AND r.kind IN ('file','symbol'))
              FOR UPDATE OF m"
        ))
        .bind(&state.project_id)
        .bind(&reported)
        .bind(&on_default)
        .fetch_all(&mut *tx)
        .await?;
        let mut cited: HashMap<String, Cited> = HashMap::new();
        for row in &rows {
            let entry = cited
                .entry(row.try_get("memory_id")?)
                .or_insert_with(|| Cited {
                    status: row.try_get("status").unwrap_or_default(),
                    files: HashSet::new(),
                    symbols: HashSet::new(),
                });
            let name: String = row.try_get("name")?;
            match row.try_get::<String, _>("kind")?.as_str() {
                "file" => entry.files.insert(name),
                _ => entry.symbols.insert(name),
            };
        }
        let (mut checked, mut stale, mut revive) = (Vec::new(), Vec::new(), Vec::new());
        for (id, m) in &cited {
            let all = |cited: &HashSet<String>, among: &HashSet<&str>| {
                cited.iter().all(|name| among.contains(name.as_str()))
            };
            let any = |cited: &HashSet<String>, among: &HashSet<&str>| {
                cited.iter().any(|name| among.contains(name.as_str()))
            };
            let looked_at = any(&m.files, &files_missing)
                || any(&m.files, &files_present)
                || any(&m.symbols, &symbols_missing)
                || any(&m.symbols, &symbols_present);
            if !looked_at {
                continue;
            }
            checked.push(id.clone());
            // A symbol alone never decides: prose words and `Type::method` names are not reliably
            // found by grep. A file in the repo that is gone, with no cited symbol still present, does.
            let gone = !m.files.is_empty()
                && all(&m.files, &files_missing)
                && (m.symbols.is_empty() || all(&m.symbols, &symbols_missing));
            let back = any(&m.files, &files_present) || any(&m.symbols, &symbols_present);
            if m.status == "active" && gone {
                stale.push(id.clone());
            } else if m.status == "stale" && back {
                revive.push(id.clone());
            }
        }
        sqlx::query("UPDATE memories SET checked_at = now() WHERE memory_id = ANY($1)")
            .bind(&checked)
            .execute(&mut *tx)
            .await?;
        for (ids, status, cause) in [
            (&stale, "stale", Some("missing")),
            (&revive, "active", None),
        ] {
            sqlx::query(
                "UPDATE memories SET status = $2, stale_cause = $3, status_changed_at = now(), updated_at = now(),
                        missing_entities = '{}'
                  WHERE memory_id = ANY($1)",
            )
            .bind(ids)
            .bind(status)
            .bind(cause)
            .execute(&mut *tx)
            .await?;
        }
        // Which cited files and symbols were found gone, for "why stale".
        for id in &stale {
            let Some(m) = cited.get(id) else { continue };
            let mut gone: Vec<&str> = m
                .files
                .iter()
                .filter(|f| files_missing.contains(f.as_str()))
                .chain(
                    m.symbols
                        .iter()
                        .filter(|s| symbols_missing.contains(s.as_str())),
                )
                .map(String::as_str)
                .collect();
            gone.sort_unstable();
            sqlx::query("UPDATE memories SET missing_entities = $2 WHERE memory_id = $1")
                .bind(id)
                .bind(&gone)
                .execute(&mut *tx)
                .await?;
        }
        // Handoffs stay; they are not promoted onto the default branch.
        let promoted: Vec<String> = sqlx::query_scalar(
            "UPDATE memories SET repo_branch = $3, updated_at = now()
              WHERE project_id = $1 AND kind <> 'handoff' AND repo_branch = ANY($2) AND owner_id = $4
              RETURNING memory_id",
        )
        .bind(&state.project_id)
        .bind(&merged)
        .bind(&state.branch)
        .bind(person_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let report = RepoStateReport {
            stale: stale.len(),
            revived: revive.len(),
            promoted: promoted.len(),
        };
        self.record_audit(
            person_id,
            None,
            "repo_state",
            Some(&state.project_id),
            serde_json::json!({
                "stale": report.stale, "revived": report.revived, "promoted": report.promoted,
                "ids": {"stale": stale, "revived": revive, "promoted": promoted},
            }),
        )
        .await?;
        Ok(report)
    }
}
