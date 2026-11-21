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
/// Serialises promotion and withdrawal so two twins promoted at once make one copy.
const PROMOTE_LOCK: &str =
    "SELECT pg_advisory_xact_lock(hashtextextended(current_schema() || '|promote', 0))";

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
        out.promoted = self.promote_memory(memory_id).await?;
        self.reconcile_promoted().await?;
        Ok(out)
    }

    /// Global promotion (CM-SPEC-008 §4.3): a project rule or gotcha written by an agent whose
    /// near-identical twin lives in another project (any owner) yields one global copy linked to
    /// both. Only `scope = project` memories of kind rule or gotcha are ever read as twins or
    /// promoted; the copy goes through the same redaction as every save. Returns the copy's id.
    pub async fn promote_memory(&self, memory_id: &str) -> Result<Option<String>> {
        let mut lock = self.pool.begin().await?;
        sqlx::query(PROMOTE_LOCK).execute(&mut *lock).await?;
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
        let twin_id: Option<String> = sqlx::query_scalar(
            "SELECT o.memory_id FROM memories s JOIN LATERAL (
                 SELECT c.memory_id, 1 - (c.embedding <=> s.embedding) AS cosine
                   FROM memories c
                  WHERE c.scope = 'project' AND c.project_id <> s.project_id AND c.kind = s.kind
                    AND c.status = 'active' AND c.origin = 'agent' AND c.embedding IS NOT NULL
                    AND (c.expires_at IS NULL OR c.expires_at > now())
                  ORDER BY c.embedding <=> s.embedding, c.memory_id LIMIT 5) o ON true
              WHERE s.memory_id = $1 AND o.cosine >= $2
              ORDER BY o.cosine DESC, o.memory_id LIMIT 1",
        )
        .bind(memory_id)
        .bind(f64::from(PROMOTE_AT))
        .fetch_optional(&mut *lock)
        .await?;
        let Some(twin_id) = twin_id else {
            return Ok(None);
        };
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
                let input = global_copy(&source, &twin);
                self.save_as(
                    &source.owner,
                    input,
                    &WorkContext::default(),
                    Origin::System,
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
            let owner: String =
                sqlx::query_scalar("SELECT owner_id FROM memories WHERE memory_id = $1")
                    .bind(&global)
                    .fetch_one(&self.pool)
                    .await?;
            self.record_audit(
                &owner,
                None,
                "memory_promoted",
                Some(&global),
                serde_json::json!({"global_id": global, "sources": [source.id, twin.id]}),
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
        let mut tx = self.pool.begin().await?;
        sqlx::query(PROMOTE_LOCK).execute(&mut *tx).await?;
        let counted = "WITH counted AS (
               SELECT g.memory_id, g.owner_id, g.expires_at,
                      (SELECT count(DISTINCT s.memory_id) FROM links l JOIN memories s ON s.memory_id = l.target_id
                        WHERE l.source_id = g.memory_id AND l.relation = 'relates' AND l.origin = 'auto'
                          AND s.scope = 'project' AND s.status IN ('active','stale')
                          AND (s.expires_at IS NULL OR s.expires_at > now())) AS live
                 FROM memories g
                WHERE g.origin = 'system' AND g.scope = 'global' AND g.kind IN ('rule','gotcha')
                  AND g.status IN ('active','stale'))";
        let withdrawn = sqlx::query(&format!(
            "{counted} UPDATE memories m SET expires_at = now(), updated_at = now() FROM counted c
              WHERE m.memory_id = c.memory_id AND c.live < 2 AND (c.expires_at IS NULL OR c.expires_at > now())
              RETURNING m.memory_id, m.owner_id, c.live"
        ))
        .fetch_all(&mut *tx)
        .await?;
        let revived = sqlx::query(&format!(
            "{counted} UPDATE memories m SET expires_at = NULL, updated_at = now() FROM counted c
              WHERE m.memory_id = c.memory_id AND c.live >= 2 AND c.expires_at <= now()
              RETURNING m.memory_id"
        ))
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        for row in &withdrawn {
            let (id, owner, live): (String, String, i64) = (
                row.try_get("memory_id")?,
                row.try_get("owner_id")?,
                row.try_get("live")?,
            );
            self.record_audit(
                &owner,
                None,
                "memory_withdrawn",
                Some(&id),
                serde_json::json!({"global_id": id, "live": live}),
            )
            .await?;
        }
        Ok((withdrawn.len(), revived.len()))
    }
}

/// The global copy: the newer source's text, the evidence of both (the redaction in `save_as`
/// runs over all of it again).
fn global_copy(a: &Memory, b: &Memory) -> SaveInput {
    let (newer, older) = if (a.created_us, &a.id) >= (b.created_us, &b.id) {
        (a, b)
    } else {
        (b, a)
    };
    let mut evidence = newer.evidence.clone();
    for item in &older.evidence {
        if evidence.len() < LIST_MAX_ITEMS && !evidence.contains(item) {
            evidence.push(item.clone());
        }
    }
    SaveInput {
        kind: newer.kind,
        title: newer.title.clone(),
        summary: newer.summary.clone(),
        reason: newer.reason.clone(),
        fields: Some(newer.fields.to_json()),
        evidence,
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
    sqlx::query("UPDATE memories SET status = 'superseded', updated_at = now() WHERE memory_id = $1 AND status IN ('active','stale')")
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
    Ok(())
}
