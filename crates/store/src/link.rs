//! The `link` job (apps/docs/content/technical/retrieval.md): automatic links from shared entities and from a gotcha's
//! fix/cause entities. Independent of the embedding model.

use crate::{Result, Store};
use cairn_core::capture::WorkContext;
use cairn_core::memory::{EntityKind, Fields, Kind, Relation};
use cairn_core::process::entities::derive;
use sqlx::{PgConnection, Row};
use std::collections::HashSet;

/// Shared-entity candidates linked per memory.
const SHARED_LIMIT: i64 = 5;
/// Memories a gotcha links to as the thing that fixes it (and the reverse).
const FIX_LIMIT: usize = 3;
/// Gotchas scanned when a decision, rule or procedure is linked.
const GOTCHA_SCAN: i64 = 500;
/// Automatic links a memory may carry before the job stops adding more.
const AUTO_LINK_CAP: i64 = 8;

/// Who `o` may be linked to for subject `$1` (owner `$2`, project `$3`): the same project, or the
/// same owner; never another person's personal memory, never a promoted global copy (its links are
/// its sources, written by promotion), and only live, non-handoff memories.
const COHORT: &str = "((o.project_id IS NOT NULL AND o.project_id = $3) OR o.owner_id = $2)
    AND (o.scope <> 'personal' OR o.owner_id = $2)
    AND NOT (o.origin = 'system' AND o.scope = 'global')
    AND o.chat_id_hash IS NOT DISTINCT FROM (SELECT chat_id_hash FROM memories WHERE memory_id = $1)
    AND o.status = 'active' AND o.kind <> 'handoff' AND o.memory_id <> $1";

/// File, symbol, command and test names of a gotcha's fix and cause text.
fn fix_names(fields: &Fields) -> Vec<(EntityKind, String)> {
    let Fields::Gotcha(gotcha) = fields else {
        return Vec::new();
    };
    derive(
        &[gotcha.fix.as_str(), gotcha.cause.as_str()],
        &WorkContext::default(),
    )
    .into_iter()
    .filter(|e| {
        matches!(
            e.kind,
            EntityKind::File | EntityKind::Symbol | EntityKind::Command | EntityKind::Test
        )
    })
    .map(|e| (e.kind, e.name))
    .collect()
}

impl Store {
    /// The `link` job body. Returns how many links were added.
    pub async fn link_memory(&self, memory_id: &str) -> Result<usize> {
        let mut tx = self.pool.begin().await?;
        crate::project::lock_memory_projects(&mut tx, &[memory_id]).await?;
        let active = crate::memory::active_project_memory("o");
        let Some(subject) = sqlx::query(
            "SELECT owner_id, project_id, kind, status, origin, scope, fields FROM memories WHERE memory_id = $1",
        )
        .bind(memory_id)
        .fetch_optional(&mut *tx)
        .await?
        else {
            return Ok(0);
        };
        let (owner, project): (String, Option<String>) =
            (subject.try_get("owner_id")?, subject.try_get("project_id")?);
        let kind: Kind = subject
            .try_get::<String, _>("kind")?
            .parse()
            .map_err(crate::StoreError::Validation)?;
        let promoted = subject.try_get::<String, _>("origin")? == "system"
            && subject.try_get::<String, _>("scope")? == "global";
        if subject.try_get::<String, _>("status")? != "active" || kind == Kind::Handoff || promoted
        {
            return Ok(0);
        }
        let fields = Fields::parse(kind, Some(subject.try_get("fields")?))?;
        let mut added = 0;
        // The memories linked and the entity that joined most of them, for the Activity page.
        let mut linked: Vec<String> = Vec::new();
        let mut cited: Vec<String> = Vec::new();

        // R2 first: the specific `fixes` relation beats the generic `relates` for the same pair.
        if kind == Kind::Gotcha {
            let (kinds, names): (Vec<&str>, Vec<String>) = fix_names(&fields)
                .into_iter()
                .map(|(k, n)| (k.as_str(), n))
                .unzip();
            let fixers: Vec<String> = sqlx::query_scalar(&format!(
                "SELECT e.memory_id FROM entities e JOIN memories o ON o.memory_id = e.memory_id
                  JOIN unnest($4::text[], $5::text[]) AS n(kind, name) ON n.kind = e.kind AND n.name = e.name
                  WHERE o.kind IN ('decision','rule','procedure') AND {COHORT} AND {active}
                  GROUP BY e.memory_id ORDER BY count(*) DESC, e.memory_id LIMIT {FIX_LIMIT}"
            ))
            .bind(memory_id)
            .bind(&owner)
            .bind(&project)
            .bind(&kinds)
            .bind(&names)
            .fetch_all(&mut *tx)
            .await?;
            for fixer in fixers {
                if insert_link(&mut tx, &fixer, memory_id, Relation::Fixes, true).await? {
                    added += 1;
                    linked.push(fixer);
                }
            }
        } else if matches!(kind, Kind::Decision | Kind::Rule | Kind::Procedure) {
            let mine: HashSet<(String, String)> = sqlx::query(
                "SELECT kind, name FROM entities WHERE memory_id = $1 AND kind IN ('file','symbol','command','test')",
            )
            .bind(memory_id)
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .map(|r| Ok((r.try_get("kind")?, r.try_get("name")?)))
            .collect::<Result<_>>()?;
            let gotchas = sqlx::query(&format!(
                "SELECT o.memory_id, o.fields FROM memories o WHERE o.kind = 'gotcha' AND {COHORT} AND {active}
                  ORDER BY o.created_at DESC, o.memory_id DESC LIMIT {GOTCHA_SCAN}"
            ))
            .bind(memory_id)
            .bind(&owner)
            .bind(&project)
            .fetch_all(&mut *tx)
            .await?;
            let mut matches: Vec<(usize, String)> = Vec::new();
            for row in gotchas {
                let fields = Fields::parse(Kind::Gotcha, Some(row.try_get("fields")?))?;
                let hits = fix_names(&fields)
                    .into_iter()
                    .filter(|(k, n)| mine.contains(&(k.as_str().to_owned(), n.clone())))
                    .count();
                if hits > 0 {
                    matches.push((hits, row.try_get("memory_id")?));
                }
            }
            matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            for (_, gotcha) in matches.into_iter().take(FIX_LIMIT) {
                if insert_link(&mut tx, memory_id, &gotcha, Relation::Fixes, true).await? {
                    added += 1;
                    linked.push(gotcha);
                }
            }
        }

        // R1: shared file, module, symbol and test entities.
        let shared = sqlx::query(&format!(
            "SELECT e2.memory_id,
                    sum(CASE WHEN e2.kind IN ('file','symbol','test') THEN 2 WHEN e2.kind = 'module' THEN 1 ELSE 0 END) AS score,
                    (array_agg(e1.name ORDER BY CASE WHEN e1.kind = 'module' THEN 1 ELSE 0 END, e1.name))[1] AS entity
               FROM entities e1
               JOIN entities e2 ON e2.name = e1.name AND e2.kind = e1.kind AND e2.memory_id <> e1.memory_id
               JOIN memories o ON o.memory_id = e2.memory_id
              WHERE e1.memory_id = $1 AND e1.kind IN ('file','module','symbol','test') AND {COHORT} AND {active}
              GROUP BY e2.memory_id ORDER BY score DESC, e2.memory_id LIMIT {SHARED_LIMIT}"
        ))
        .bind(memory_id)
        .bind(&owner)
        .bind(&project)
        .fetch_all(&mut *tx)
        .await?;
        for row in shared {
            let (other, score): (String, i64) = (row.try_get("memory_id")?, row.try_get("score")?);
            if score >= 1
                && insert_link(&mut tx, memory_id, &other, Relation::Relates, true).await?
            {
                added += 1;
                cited.push(row.try_get("entity")?);
                linked.push(other);
            }
        }
        tx.commit().await?;
        if added > 0 {
            let mut counts = std::collections::BTreeMap::<&str, usize>::new();
            for name in &cited {
                *counts.entry(name.as_str()).or_default() += 1;
            }
            // Most cited first, then by name, so the same links always say the same sentence.
            let entity = counts
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
                .map(|(name, _)| name);
            self.record_audit(
                &owner,
                None,
                "links_added",
                Some(memory_id),
                serde_json::json!({"count": added, "entity": entity, "ids": linked}),
            )
            .await?;
        }
        Ok(added)
    }
}

/// Adds one automatic link unless the pair is already linked (either direction, any relation) or,
/// when `capped`, either memory already carries 8 automatic links. True when a row was written.
pub(crate) async fn insert_link(
    tx: &mut PgConnection,
    source: &str,
    target: &str,
    relation: Relation,
    capped: bool,
) -> Result<bool> {
    if source == target {
        return Ok(false);
    }
    crate::project::lock_memory_projects(tx, &[source, target]).await?;
    let linked: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM links WHERE (source_id = $1 AND target_id = $2) OR (source_id = $2 AND target_id = $1))",
    )
    .bind(source)
    .bind(target)
    .fetch_one(&mut *tx)
    .await?;
    if linked {
        return Ok(false);
    }
    if capped {
        let full: bool = sqlx::query_scalar(&format!(
            "SELECT (SELECT count(*) FROM links WHERE origin = 'auto' AND (source_id = $1 OR target_id = $1)) >= {AUTO_LINK_CAP}
                 OR (SELECT count(*) FROM links WHERE origin = 'auto' AND (source_id = $2 OR target_id = $2)) >= {AUTO_LINK_CAP}"
        ))
        .bind(source)
        .bind(target)
        .fetch_one(&mut *tx)
        .await?;
        if full {
            return Ok(false);
        }
    }
    let done = sqlx::query(
        "INSERT INTO links (source_id, target_id, relation, origin) VALUES ($1, $2, $3, 'auto')
         ON CONFLICT (source_id, target_id, relation) DO NOTHING",
    )
    .bind(source)
    .bind(target)
    .bind(relation.as_str())
    .execute(tx)
    .await?;
    Ok(done.rows_affected() == 1)
}
