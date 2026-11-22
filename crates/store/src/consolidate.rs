use crate::link::insert_link;
use crate::memory::{MEMORY_SELECT, memory_from_row};
use crate::{Result, Store};
use cairn_core::capture::WorkContext;
use cairn_core::memory::identity::evidence_hash;
use cairn_core::memory::validate::LIST_MAX_ITEMS;
use cairn_core::memory::{Memory, Relation, SaveInput, Scope};
use cairn_core::process::Origin;
use cairn_core::process::consolidate::{Band, CANDIDATES, SUPERSEDE_AT, Side, band, guard};
use sqlx::{PgConnection, Row};

/// What one consolidation changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Consolidated {
    /// Older memories now `superseded` by a newer one.
    pub superseded: Vec<String>,
    /// Memories that got an automatic `relates` link.
    pub related: Vec<String>,
    /// The global copy this memory's promotion created or joined, if any.
    pub promoted: Option<String>,
}

/// A twin in another project: a rule or gotcha this close in meaning counts as "the same lesson".
const PROMOTE_AT: f32 = SUPERSEDE_AT;
/// The owner of every promoted copy: no person wrote it.
pub(crate) const SYSTEM_OWNER: &str = "system";
/// Kinds that are promoted; a lock per kind serialises promotion and withdrawal of one kind so two
/// twins promoted at once make one copy, and nothing else waits.
const PROMOTED_KINDS: [&str; 2] = ["gotcha", "rule"];

async fn lock_promotion(tx: &mut PgConnection, kind: &str) -> Result<()> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended(current_schema() || '|promote|' || $1, 0))",
    )
    .bind(kind)
    .execute(tx)
    .await?;
    Ok(())
}

/// The reserved account that owns promoted copies. Created on first use; it cannot log in.
async fn ensure_system_owner(pool: &sqlx::PgPool) -> Result<()> {
    sqlx::query(
        "INSERT INTO accounts (person_id, email, password_hash, status)
         VALUES ($1, 'system@cairn.invalid', '!', 'disabled') ON CONFLICT DO NOTHING",
    )
    .bind(SYSTEM_OWNER)
    .execute(pool)
    .await?;
    Ok(())
}

impl Store {
    /// The `consolidate` job body. Takes the 20 nearest neighbours with the same owner, scope,
    /// project and kind (the cohort is selected first, then ranked exactly). Cosine >= 0.92: the
    /// newer supersedes the older, evidence is unioned onto the newer, and the older stays in
    /// history as `superseded`. 0.75..0.92: an automatic `relates` link. A declared `contradicts`
    /// link between two memories is never overridden.
    pub async fn consolidate_memory(&self, memory_id: &str) -> Result<Consolidated> {
        let mut out = Consolidated::default();
        let mut tx = self.pool.begin().await?;
        // One consolidation per cohort at a time: two jobs on a mutual near-duplicate pair each
        // update the other's row and would deadlock.
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended(
                        concat_ws('|', current_schema(), owner_id, scope, coalesce(project_id, ''), kind), 0))
               FROM memories WHERE memory_id = $1",
        )
        .bind(memory_id)
        .execute(&mut *tx)
        .await?;
        let subject = sqlx::query(
            "SELECT owner_id, scope, project_id, kind, status, repo_branch, origin, embedding::text AS embedding
               FROM memories WHERE memory_id = $1 AND embedding IS NOT NULL FOR UPDATE",
        )
        .bind(memory_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(subject) = subject else {
            return Ok(out);
        };
        let kind: String = subject.try_get("kind")?;
        let status: String = subject.try_get("status")?;
        if kind == "handoff" || !matches!(status.as_str(), "active" | "stale") {
            return Ok(out);
        }
        let vector: String = subject.try_get("embedding")?;
        let subject_branch: Option<String> = subject.try_get("repo_branch")?;
        let subject_origin: Origin = subject
            .try_get::<String, _>("origin")?
            .parse()
            .map_err(crate::StoreError::Validation)?;
        // The cohort is materialised before the distance sort, so no hnsw scan can cut the
        // window to other people's nearest vectors first.
        let neighbours = sqlx::query(
            "WITH cohort AS MATERIALIZED (
                 SELECT m.memory_id, m.created_at, m.embedding, m.repo_branch, m.origin
                   FROM memories m
                  WHERE m.embedding IS NOT NULL AND m.owner_id = $2 AND m.scope = $3 AND m.project_id IS NOT DISTINCT FROM $4 AND m.kind = $5
                    AND m.memory_id <> $6 AND m.status IN ('active','stale')
                    AND (m.expires_at IS NULL OR m.expires_at > now()))
             SELECT c.memory_id, c.repo_branch, c.origin, 1 - (c.embedding <=> $1::vector) AS cosine,
                    (c.created_at, c.memory_id) > (s.created_at, s.memory_id) AS neighbour_newer
               FROM cohort c JOIN memories s ON s.memory_id = $6
              ORDER BY c.embedding <=> $1::vector, c.memory_id LIMIT $7",
        )
        .bind(&vector)
        .bind(subject.try_get::<String, _>("owner_id")?)
        .bind(subject.try_get::<String, _>("scope")?)
        .bind(subject.try_get::<Option<String>, _>("project_id")?)
        .bind(&kind)
        .bind(memory_id)
        .bind(CANDIDATES as i64)
        .fetch_all(&mut *tx)
        .await?;
        let mut subject_superseded = false;
        let subject_owner: String = subject.try_get("owner_id")?;
        // Newer memory -> the duplicates it absorbed, for one `memory_merged` event each.
        let mut merged: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for row in neighbours {
            let neighbour: String = row.try_get("memory_id")?;
            let cosine = row.try_get::<f64, _>("cosine")? as f32;
            let Some(band) = band(cosine) else { continue };
            if declared_contradiction(&mut tx, memory_id, &neighbour).await? {
                continue;
            }
            let neighbour_newer: bool = row.try_get("neighbour_newer")?;
            let neighbour_branch: Option<String> = row.try_get("repo_branch")?;
            let neighbour_origin: Origin = row
                .try_get::<String, _>("origin")?
                .parse()
                .map_err(crate::StoreError::Validation)?;
            let subject_side = Side {
                branch: subject_branch.as_deref(),
                origin: subject_origin,
            };
            let neighbour_side = Side {
                branch: neighbour_branch.as_deref(),
                origin: neighbour_origin,
            };
            let band = if neighbour_newer {
                guard(band, neighbour_side, subject_side)
            } else {
                guard(band, subject_side, neighbour_side)
            };
            match band {
                Band::Supersede if !subject_superseded => {
                    let (newer, older) = if neighbour_newer {
                        (neighbour.as_str(), memory_id)
                    } else {
                        (memory_id, neighbour.as_str())
                    };
                    supersede(&mut tx, newer, older).await?;
                    subject_superseded = older == memory_id;
                    out.superseded.push(older.to_owned());
                    merged
                        .entry(newer.to_owned())
                        .or_default()
                        .push(older.to_owned());
                }
                Band::Supersede => {}
                Band::Relate => {
                    let done = sqlx::query(
                        "INSERT INTO links (source_id, target_id, relation, origin)
                         SELECT $1, $2, 'relates', 'auto'
                          WHERE NOT EXISTS (SELECT 1 FROM links WHERE relation = 'relates'
                                             AND ((source_id = $1 AND target_id = $2) OR (source_id = $2 AND target_id = $1)))",
                    )
                    .bind(memory_id)
                    .bind(&neighbour)
                    .execute(&mut *tx)
                    .await?;
                    if done.rows_affected() == 1 {
                        out.related.push(neighbour);
                    }
                }
            }
        }
        tx.commit().await?;
        for (into, older) in merged.into_iter().filter(|_| subject_owner != SYSTEM_OWNER) {
            self.record_audit(
                &subject_owner,
                None,
                "memory_merged",
                Some(&into),
                serde_json::json!({"into": into, "merged": older}),
            )
            .await?;
        }
        out.promoted = self.promote_memory(memory_id).await?;
        // A copy's source count only changes when something is promoted or superseded here; the
        // sweep covers every other change.
        if out.promoted.is_some() || !out.superseded.is_empty() {
            self.reconcile_promoted().await?;
        }
        Ok(out)
    }

    /// Global promotion (CM-SPEC-008 §4.3): a project rule or gotcha written by an agent whose
    /// near-identical twin lives in another project (any owner) yields one global copy linked to
    /// both. Only `scope = project` memories of kind rule or gotcha are ever read as twins or
    /// promoted; the copy goes through the same redaction as every save. Returns the copy's id.
    pub async fn promote_memory(&self, memory_id: &str) -> Result<Option<String>> {
        let mut lock = self.pool.begin().await?;
        // The same iterative scan as retrieval: the visibility filters below must not run only
        // over the index's first `ef_search` hits.
        sqlx::query(
            "SELECT set_config('hnsw.iterative_scan', 'relaxed_order', true),
                    set_config('hnsw.ef_search', '100', true),
                    set_config('hnsw.max_scan_tuples', '20000', true)",
        )
        .execute(&mut *lock)
        .await?;
        let rows = sqlx::query(&format!(
            "{MEMORY_SELECT} WHERE m.memory_id = $1 AND m.scope = 'project' AND m.kind IN ('rule','gotcha')
                AND m.origin = 'agent' AND m.status = 'active' AND m.embedding IS NOT NULL
                AND (m.expires_at IS NULL OR m.expires_at > now())"
        ))
        .bind(memory_id)
        .fetch_all(&mut *lock)
        .await?;
        let Some(row) = rows.first() else {
            return Ok(None);
        };
        let source = memory_from_row(row)?;
        let (vector, project): (String, String) =
            sqlx::query_as("SELECT embedding::text, project_id FROM memories WHERE memory_id = $1")
                .bind(memory_id)
                .fetch_one(&mut *lock)
                .await?;
        // The same shape as the retrieval vector channel (a materialised CTE ordered by a bound
        // vector, with iterative scan on) so the filters never run over the index's first hits only.
        let twin_id: Option<String> = sqlx::query_scalar(
            "WITH c AS MATERIALIZED (
                 SELECT m.memory_id, m.embedding <=> $1::vector AS dist
                   FROM memories m
                  WHERE m.embedding IS NOT NULL AND m.scope = 'project' AND m.project_id <> $2
                    AND m.kind = $3 AND m.status = 'active' AND m.origin = 'agent'
                    AND (m.expires_at IS NULL OR m.expires_at > now())
                  ORDER BY m.embedding <=> $1::vector LIMIT 5)
             SELECT memory_id FROM c WHERE 1 - dist >= $4 ORDER BY dist, memory_id LIMIT 1",
        )
        .bind(&vector)
        .bind(&project)
        .bind(source.kind.as_str())
        .bind(f64::from(PROMOTE_AT))
        .fetch_optional(&mut *lock)
        .await?;
        let Some(twin_id) = twin_id else {
            return Ok(None);
        };
        // Only a memory with a twin takes the lock, and only against promotions of its own kind.
        lock_promotion(&mut lock, source.kind.as_str()).await?;
        let twin = memory_from_row(
            &sqlx::query(&format!("{MEMORY_SELECT} WHERE m.memory_id = $1"))
                .bind(&twin_id)
                .fetch_one(&mut *lock)
                .await?,
        )?;
        // An existing copy (live or withdrawn) is reused: first the one already linked to either
        // source (it may not be embedded yet), then the nearest embedded one.
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT g.memory_id FROM memories g
              WHERE g.scope = 'global' AND g.origin = 'system' AND g.kind = $3 AND g.status IN ('active','stale')
                AND (EXISTS (SELECT 1 FROM links l WHERE l.source_id = g.memory_id AND l.target_id = ANY($4))
                     OR (g.embedding IS NOT NULL
                         AND 1 - (g.embedding <=> (SELECT embedding FROM memories WHERE memory_id = $1)) >= $2))
              ORDER BY EXISTS (SELECT 1 FROM links l WHERE l.source_id = g.memory_id AND l.target_id = ANY($4)) DESC,
                       g.memory_id LIMIT 1",
        )
        .bind(memory_id)
        .bind(f64::from(PROMOTE_AT))
        .bind(source.kind.as_str())
        .bind([source.id.as_str(), twin.id.as_str()])
        .fetch_optional(&mut *lock)
        .await?;
        let global = match existing {
            Some(id) => id,
            None => {
                // Committed first: the copy is saved on another connection.
                ensure_system_owner(&self.pool).await?;
                let input = global_copy(&source, &twin);
                self.save_as(
                    SYSTEM_OWNER,
                    input,
                    &WorkContext::default(),
                    Origin::System,
                    None,
                )
                .await?
                .id
            }
        };
        let mut added = 0;
        for target in [&source.id, &twin.id] {
            added += usize::from(
                insert_link(&mut lock, &global, target, Relation::Relates, false).await?,
            );
        }
        lock.commit().await?;
        if added > 0 {
            self.record_audit(
                &source.owner,
                None,
                "memory_promoted",
                Some(&global),
                serde_json::json!({"global_id": global, "sources": [source.id, twin.id], "ids": [source.id, twin.id]}),
            )
            .await?;
        }
        Ok(Some(global))
    }

    /// Withdraws every promoted global copy that has fewer than 2 live sources (`expires_at = now()`
    /// hides it from every read) and brings back one whose two live sources returned. A source is
    /// live when the copy links to it, it is project-scoped, `active` or `stale`, unexpired and not
    /// forgotten. Safe to run often. Returns `(withdrawn, revived)`.
    pub async fn reconcile_promoted(&self) -> Result<(usize, usize)> {
        let (mut withdrawn_total, mut revived_total) = (0, 0);
        for kind in PROMOTED_KINDS {
            let mut tx = self.pool.begin().await?;
            lock_promotion(&mut tx, kind).await?;
            let counted = "WITH counted AS (
               SELECT g.memory_id, g.expires_at,
                      (SELECT count(DISTINCT s.memory_id) FROM links l JOIN memories s ON s.memory_id = l.target_id
                        WHERE l.source_id = g.memory_id AND l.relation = 'relates' AND l.origin = 'auto'
                          AND s.scope = 'project' AND s.status IN ('active','stale')
                          AND (s.expires_at IS NULL OR s.expires_at > now())) AS live
                 FROM memories g
                WHERE g.origin = 'system' AND g.scope = 'global' AND g.kind = $1
                  AND g.status IN ('active','stale'))";
            let withdrawn = sqlx::query(&format!(
                "{counted} UPDATE memories m SET expires_at = now(), updated_at = now() FROM counted c
                  WHERE m.memory_id = c.memory_id AND c.live < 2 AND (c.expires_at IS NULL OR c.expires_at > now())
                  RETURNING m.memory_id, c.live"
            ))
            .bind(kind)
            .fetch_all(&mut *tx)
            .await?;
            // Only an active copy comes back; one flagged wrong (stale) lapses with the age sweep.
            let revived = sqlx::query(&format!(
                "{counted} UPDATE memories m SET expires_at = NULL, updated_at = now() FROM counted c
                  WHERE m.memory_id = c.memory_id AND m.status = 'active' AND c.live >= 2 AND c.expires_at <= now()
                  RETURNING m.memory_id"
            ))
            .bind(kind)
            .fetch_all(&mut *tx)
            .await?;
            tx.commit().await?;
            for row in &withdrawn {
                let (id, live): (String, i64) = (row.try_get("memory_id")?, row.try_get("live")?);
                // The copy has no person as owner: its sources' owners see the withdrawal.
                let owners: Vec<String> = sqlx::query_scalar(
                    "SELECT DISTINCT s.owner_id FROM links l JOIN memories s ON s.memory_id = l.target_id
                      WHERE l.source_id = $1 AND l.relation = 'relates' AND l.origin = 'auto'",
                )
                .bind(&id)
                .fetch_all(&self.pool)
                .await?;
                for owner in owners {
                    self.record_audit(
                        &owner,
                        None,
                        "memory_withdrawn",
                        Some(&id),
                        serde_json::json!({"global_id": id, "live": live}),
                    )
                    .await?;
                }
            }
            withdrawn_total += withdrawn.len();
            revived_total += revived.len();
        }
        Ok((withdrawn_total, revived_total))
    }
}

/// The global copy: the newer source's text only. No evidence is copied (paths, commits and URLs
/// belong to a project; the hidden links to the sources already record where it came from), and
/// the redaction in `save_as` runs over the text again.
fn global_copy(a: &Memory, b: &Memory) -> SaveInput {
    let newer = if (a.created_us, &a.id) >= (b.created_us, &b.id) {
        a
    } else {
        b
    };
    SaveInput {
        kind: newer.kind,
        title: newer.title.clone(),
        summary: newer.summary.clone(),
        reason: newer.reason.clone(),
        fields: Some(newer.fields.to_json()),
        evidence: Vec::new(),
        supersedes: None,
        contradicts: None,
        scope: Some(Scope::Global),
    }
}

async fn declared_contradiction(tx: &mut PgConnection, a: &str, b: &str) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM links WHERE relation = 'contradicts'
                         AND ((source_id = $1 AND target_id = $2) OR (source_id = $2 AND target_id = $1)))",
    )
    .bind(a)
    .bind(b)
    .fetch_one(tx)
    .await?)
}

/// `older` becomes `superseded`; `newer` keeps its text and gains the older's evidence and count.
async fn supersede(tx: &mut PgConnection, newer: &str, older: &str) -> Result<()> {
    let rows = sqlx::query(
        "SELECT memory_id, evidence, evidence_count FROM memories WHERE memory_id = ANY($1)",
    )
    .bind([newer, older])
    .fetch_all(&mut *tx)
    .await?;
    let (mut newer_evidence, mut older_evidence) = (Vec::<String>::new(), Vec::<String>::new());
    let (mut newer_count, mut older_count) = (0_i32, 0_i32);
    for row in rows {
        let (evidence, count) = (
            row.try_get::<Vec<String>, _>("evidence")?,
            row.try_get::<i32, _>("evidence_count")?,
        );
        if row.try_get::<String, _>("memory_id")? == newer {
            (newer_evidence, newer_count) = (evidence, count);
        } else {
            (older_evidence, older_count) = (evidence, count);
        }
    }
    for item in older_evidence {
        if newer_evidence.len() < LIST_MAX_ITEMS && !newer_evidence.contains(&item) {
            newer_evidence.push(item);
        }
    }
    sqlx::query("UPDATE memories SET evidence = $2, evidence_count = $3, evidence_hash = $4, updated_at = now() WHERE memory_id = $1")
        .bind(newer)
        .bind(&newer_evidence)
        .bind(newer_count + older_count)
        .bind(evidence_hash(&newer_evidence))
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE memories SET status = 'superseded', stale_cause = NULL, status_changed_at = now(), updated_at = now() WHERE memory_id = $1 AND status IN ('active','stale')")
        .bind(older)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO links (source_id, target_id, relation, origin) VALUES ($1, $2, 'supersedes', 'auto')
         ON CONFLICT (source_id, target_id, relation) DO NOTHING",
    )
    .bind(newer)
    .bind(older)
    .execute(&mut *tx)
    .await?;
    // The newer side restates the older one, which confirms it.
    let confirmed = [newer.to_owned()];
    crate::memory::promote_candidates(&mut *tx, &confirmed).await?;
    Ok(())
}
