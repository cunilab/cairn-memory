use crate::{Result, Store};
use cairn_core::memory::identity::evidence_hash;
use cairn_core::memory::validate::LIST_MAX_ITEMS;
use cairn_core::process::consolidate::{Band, CANDIDATES, band};
use sqlx::{PgConnection, Row};

/// What one consolidation changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Consolidated {
    /// Older memories now `superseded` by a newer one.
    pub superseded: Vec<String>,
    /// Memories that got an automatic `relates` link.
    pub related: Vec<String>,
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
            "SELECT owner_id, scope, project_id, kind, status, embedding::text AS embedding
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
        // The cohort is materialised before the distance sort, so no hnsw scan can cut the
        // window to other people's nearest vectors first.
        let neighbours = sqlx::query(
            "WITH cohort AS MATERIALIZED (
                 SELECT m.memory_id, m.created_at, m.embedding
                   FROM memories m
                  WHERE m.embedding IS NOT NULL AND m.owner_id = $2 AND m.scope = $3 AND m.project_id IS NOT DISTINCT FROM $4 AND m.kind = $5
                    AND m.memory_id <> $6 AND m.status IN ('active','stale')
                    AND (m.expires_at IS NULL OR m.expires_at > now()))
             SELECT c.memory_id, 1 - (c.embedding <=> $1::vector) AS cosine,
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
            match band {
                Band::Supersede if !subject_superseded => {
                    let (newer, older) = if row.try_get("neighbour_newer")? {
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
        Ok(out)
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
