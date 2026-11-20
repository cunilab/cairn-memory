use crate::embed::vector_text;
use crate::memory::{MEMORY_SELECT, memory_from_row, visible};
use crate::{Result, Store};
use cairn_core::capture::WorkContext;
use cairn_core::memory::{Fields, Kind, Memory, Status};
use cairn_core::process::embed::validate_vector;
use cairn_core::process::entities;
use cairn_core::retrieve::brief::session_brief;
use cairn_core::retrieve::filter::{Filter, matches, passes_relevance_floor, tokenize};
use cairn_core::retrieve::graph::{Graph, GraphFilter, build, node_cap};
use cairn_core::retrieve::hybrid::{CANDIDATES_PER_CHANNEL, fuse, kind_weight};
use cairn_core::retrieve::pack::{Hit, Pack, pack};
use cairn_core::retrieve::rank::multiplier;
use serde::Serialize;
use sqlx::Row;
use std::collections::{BTreeMap, HashMap};

/// Hits kept after fusion, before the request filter and floor trim them.
const POOL: usize = 200;

/// How `rank` treats the result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Agent search and context: the relevance floor applies.
    Search,
    /// Why-changed: no floor; decisions and replaced memories come first.
    WhyChanged,
}

/// Counts for `GET /diagnose`: what the person can see, and their own share of the job queue.
#[derive(Clone, Debug, Serialize)]
pub struct Diagnostics {
    pub memories: i64,
    pub by_kind: BTreeMap<String, i64>,
    pub by_status: BTreeMap<String, i64>,
    pub by_scope: BTreeMap<String, i64>,
    pub jobs_pending: i64,
    pub jobs_running: i64,
    pub jobs_quarantined: i64,
}

/// Candidate channels for hybrid search. Each returns `(memory_id, score)` for memories the
/// person may see; fusion is `cairn_core::retrieve::hybrid::fuse`.
impl Store {
    /// Every memory the person can see, with entities and links. Step 5 replaces this for search.
    pub async fn visible_memories(&self, person_id: &str) -> Result<Vec<Memory>> {
        let rows = sqlx::query(&format!(
            "{MEMORY_SELECT} WHERE {} ORDER BY m.updated_at DESC, m.memory_id DESC",
            visible("m", "$1")
        ))
        .bind(person_id)
        .fetch_all(&self.pool)
        .await?;
        let mut memories = rows
            .iter()
            .map(memory_from_row)
            .collect::<Result<Vec<_>>>()?;
        self.attach(person_id, &mut memories).await?;
        Ok(memories)
    }

    /// Full-text channel: `websearch_to_tsquery('english', q)` over `memories_search_idx`.
    pub async fn lexical_candidates(
        &self,
        person_id: &str,
        query: &str,
        limit: i64,
    ) -> Result<Vec<(String, f32)>> {
        scored(
            sqlx::query(&format!(
                "SELECT m.memory_id, ts_rank_cd(m.search_tsv, q.query)::float4 AS score
                   FROM memories m, websearch_to_tsquery('english', $2) AS q(query)
                  WHERE m.search_tsv @@ q.query AND {}
                  ORDER BY score DESC, m.memory_id LIMIT $3",
                visible("m", "$1")
            ))
            .bind(person_id)
            .bind(query)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?,
        )
    }

    /// Entity channel: score is the number of matching entity names, via `entities_lookup_idx`.
    pub async fn entity_candidates(
        &self,
        person_id: &str,
        names: &[String],
        limit: i64,
    ) -> Result<Vec<(String, f32)>> {
        scored(
            sqlx::query(&format!(
                "SELECT e.memory_id, count(*)::float4 AS score
                   FROM entities e JOIN memories m ON m.memory_id = e.memory_id
                  WHERE e.name = ANY($2) AND {}
                  GROUP BY e.memory_id ORDER BY score DESC, e.memory_id LIMIT $3",
                visible("m", "$1")
            ))
            .bind(person_id)
            .bind(names)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?,
        )
    }

    /// Semantic channel: the nearest vectors by cosine through `memory_embeddings_hnsw`, then the
    /// visibility filter. Score is cosine similarity.
    pub async fn vector_candidates(
        &self,
        person_id: &str,
        vector: &[f32],
        limit: i64,
    ) -> Result<Vec<(String, f32)>> {
        validate_vector(vector)?;
        // hnsw.ef_search defaults to 40 and caps the rows an index scan returns, so a larger
        // LIMIT would silently come back short.
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT set_config('hnsw.ef_search', $1, true)")
            .bind(limit.clamp(40, 1000).to_string())
            .execute(&mut *tx)
            .await?;
        let rows = sqlx::query(&format!(
            "SELECT c.memory_id, (1 - c.dist)::float4 AS score
               FROM (SELECT memory_id, embedding <=> $2::vector AS dist
                       FROM memory_embeddings ORDER BY embedding <=> $2::vector LIMIT $3) c
               JOIN memories m ON m.memory_id = c.memory_id
              WHERE {} ORDER BY c.dist, c.memory_id",
            visible("m", "$1")
        ))
        .bind(person_id)
        .bind(vector_text(vector))
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        scored(rows)
    }
}

fn scored(rows: Vec<sqlx::postgres::PgRow>) -> Result<Vec<(String, f32)>> {
    rows.iter()
        .map(|r| Ok((r.try_get("memory_id")?, r.try_get("score")?)))
        .collect()
}

impl Store {
    /// Visible memories by id, with entities and links. Unknown and hidden ids are absent.
    pub async fn memories_by_ids(&self, person_id: &str, ids: &[String]) -> Result<Vec<Memory>> {
        let rows = sqlx::query(&format!(
            "{MEMORY_SELECT} WHERE m.memory_id = ANY($2) AND {}",
            visible("m", "$1")
        ))
        .bind(person_id)
        .bind(ids)
        .fetch_all(&self.pool)
        .await?;
        let mut memories = rows
            .iter()
            .map(memory_from_row)
            .collect::<Result<Vec<_>>>()?;
        self.attach(person_id, &mut memories).await?;
        Ok(memories)
    }

    /// Hybrid search. Every candidate channel, the neighbour expansion and the final load apply
    /// the visibility rule, so no hit can come from another person's private memory. The vector
    /// channel takes its top-K before that rule; step 5 moves the rule inside it.
    pub async fn search(
        &self,
        person_id: &str,
        query: &str,
        vector: Option<&[f32]>,
        limit: usize,
        filter: &Filter,
    ) -> Result<Pack> {
        let hits = self
            .rank(person_id, query, vector, filter, Mode::Search)
            .await?;
        self.packed(query, hits, limit).await
    }

    pub async fn why_changed(
        &self,
        person_id: &str,
        query: &str,
        vector: Option<&[f32]>,
        limit: usize,
        filter: &Filter,
    ) -> Result<Pack> {
        let hits = self
            .rank(person_id, query, vector, filter, Mode::WhyChanged)
            .await?;
        self.packed(&format!("why changed: {query}"), hits, limit)
            .await
    }

    async fn packed(&self, query: &str, hits: Vec<Hit>, limit: usize) -> Result<Pack> {
        let pack = pack(query, hits, limit);
        let ids: Vec<String> = pack.results.iter().map(|h| h.memory.id.clone()).collect();
        // An agent read counts as a fetch; the person's own pages use `get` and `list`.
        self.mark_fetched(&ids).await?;
        Ok(pack)
    }

    async fn rank(
        &self,
        person_id: &str,
        query: &str,
        vector: Option<&[f32]>,
        filter: &Filter,
        mode: Mode,
    ) -> Result<Vec<Hit>> {
        if query.trim().is_empty() {
            return Err(crate::StoreError::Validation(
                "query must not be empty".into(),
            ));
        }
        let per_channel = CANDIDATES_PER_CHANNEL as i64;
        let lexical = self
            .lexical_candidates(person_id, query, per_channel)
            .await?;
        let names = query_entity_names(query, filter.context.as_ref());
        let entity = self
            .entity_candidates(person_id, &names, per_channel)
            .await?;
        let semantic = match vector {
            Some(vector) => {
                self.vector_candidates(person_id, vector, per_channel)
                    .await?
            }
            None => Vec::new(),
        };
        let seeds = fuse(vec![lexical.clone(), entity.clone(), semantic.clone()]);
        let neighbours = self.neighbours(person_id, &seeds).await?;
        let mut fused = fuse(vec![lexical, entity, semantic, neighbours]);
        fused.truncate(POOL);
        let ids: Vec<String> = fused.iter().map(|(id, _)| id.clone()).collect();
        let by_id: HashMap<String, Memory> = self
            .memories_by_ids(person_id, &ids)
            .await?
            .into_iter()
            .map(|m| (m.id.clone(), m))
            .collect();
        let mut hits: Vec<Hit> = fused
            .into_iter()
            .filter_map(|(id, score)| {
                let memory = by_id.get(&id)?.clone();
                let eligible = memory.status != Status::Superseded
                    && matches(&memory, filter)
                    && (mode == Mode::WhyChanged || passes_relevance_floor(&memory, query));
                eligible.then(|| Hit {
                    score: score * kind_weight(memory.kind) * multiplier(&memory.usage),
                    memory,
                })
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.memory.id.cmp(&b.memory.id))
        });
        if mode == Mode::WhyChanged {
            // A stable sort keeps score order inside each class.
            hits.sort_by_key(|h| std::cmp::Reverse(change_class(&h.memory)));
        }
        Ok(hits)
    }

    /// One hop from the fused seeds over `links`: each linked memory the person can see gets the
    /// summed score of its seeds, as a ranked channel.
    async fn neighbours(
        &self,
        person_id: &str,
        seeds: &[(String, f32)],
    ) -> Result<Vec<(String, f32)>> {
        if seeds.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<String> = seeds
            .iter()
            .take(CANDIDATES_PER_CHANNEL)
            .map(|(id, _)| id.clone())
            .collect();
        let rows = sqlx::query(&format!(
            "SELECT l.source_id, l.target_id FROM links l
               JOIN memories n ON n.memory_id = CASE WHEN l.source_id = ANY($2) THEN l.target_id ELSE l.source_id END
              WHERE (l.source_id = ANY($2) OR l.target_id = ANY($2)) AND {}",
            visible("n", "$1")
        ))
        .bind(person_id)
        .bind(&ids)
        .fetch_all(&self.pool)
        .await?;
        let score: HashMap<&str, f32> = seeds.iter().map(|(id, s)| (id.as_str(), *s)).collect();
        let mut sum: HashMap<String, f32> = HashMap::new();
        for row in &rows {
            let (source, target): (String, String) =
                (row.try_get("source_id")?, row.try_get("target_id")?);
            for (seed, other) in [(&source, &target), (&target, &source)] {
                if let Some(s) = score.get(seed.as_str())
                    && ids.contains(seed)
                {
                    *sum.entry(other.clone()).or_default() += s;
                }
            }
        }
        for id in &ids {
            sum.remove(id);
        }
        Ok(sum.into_iter().collect())
    }

    /// The person's live handoffs, newest first, narrowed by the filter.
    pub async fn resume(&self, person_id: &str, limit: usize, filter: &Filter) -> Result<Pack> {
        let handoffs = self.handoffs(person_id, filter).await?;
        Ok(pack("resume", unit_hits(handoffs), limit))
    }

    /// Handoffs that still list next steps or open questions.
    pub async fn unresolved(&self, person_id: &str, limit: usize, filter: &Filter) -> Result<Pack> {
        let open: Vec<Memory> = self
            .handoffs(person_id, filter)
            .await?
            .into_iter()
            .filter(|m| {
                matches!(&m.fields, Fields::Handoff(h)
                    if !h.next_steps.is_empty() || !h.open_questions.is_empty())
            })
            .collect();
        Ok(pack("unresolved", unit_hits(open), limit))
    }

    async fn handoffs(&self, person_id: &str, filter: &Filter) -> Result<Vec<Memory>> {
        let rows = sqlx::query(&format!(
            "{MEMORY_SELECT}
              WHERE m.owner_id = $1 AND m.kind = 'handoff' AND m.status = 'active' AND {}
              ORDER BY m.updated_at DESC, m.memory_id DESC LIMIT 200",
            visible("m", "$1")
        ))
        .bind(person_id)
        .fetch_all(&self.pool)
        .await?;
        let mut memories = rows
            .iter()
            .map(memory_from_row)
            .collect::<Result<Vec<_>>>()?;
        self.attach(person_id, &mut memories).await?;
        memories.retain(|m| matches(m, filter));
        Ok(memories)
    }

    /// The fenced brief a hook injects at session start.
    pub async fn brief(&self, person_id: &str, context: &WorkContext) -> Result<String> {
        let rows = sqlx::query(&format!(
            "{MEMORY_SELECT}
              WHERE {} AND m.status = 'active'
                AND ((m.kind IN ('rule','decision') AND m.scope = 'project' AND m.project_id = $2)
                     OR (m.kind IN ('preference','handoff') AND m.owner_id = $1))
              ORDER BY m.updated_at DESC, m.memory_id DESC LIMIT 500",
            visible("m", "$1")
        ))
        .bind(person_id)
        .bind(&context.project_id)
        .fetch_all(&self.pool)
        .await?;
        let memories = rows
            .iter()
            .map(memory_from_row)
            .collect::<Result<Vec<_>>>()?;
        Ok(session_brief(&memories, context))
    }

    /// Nodes are the memories the person can see; edges are the links between them.
    pub async fn graph(
        &self,
        person_id: &str,
        filter: &GraphFilter,
        cap: Option<usize>,
    ) -> Result<Graph> {
        let cap = node_cap(cap)?;
        let memories = self.visible_memories(person_id).await?;
        let links: Vec<_> = memories.iter().flat_map(|m| m.links.clone()).collect();
        Ok(build(&memories, &links, filter, cap))
    }

    pub async fn diagnose(&self, person_id: &str) -> Result<Diagnostics> {
        let mut out = Diagnostics {
            memories: 0,
            by_kind: BTreeMap::new(),
            by_status: BTreeMap::new(),
            by_scope: BTreeMap::new(),
            jobs_pending: 0,
            jobs_running: 0,
            jobs_quarantined: 0,
        };
        for (column, map) in [
            ("kind", &mut out.by_kind),
            ("status", &mut out.by_status),
            ("scope", &mut out.by_scope),
        ] {
            let rows = sqlx::query(&format!(
                "SELECT m.{column} AS k, count(*) AS n FROM memories m WHERE {} GROUP BY m.{column}",
                visible("m", "$1")
            ))
            .bind(person_id)
            .fetch_all(&self.pool)
            .await?;
            for row in rows {
                map.insert(row.try_get("k")?, row.try_get("n")?);
            }
        }
        out.memories = out.by_status.values().sum();
        (out.jobs_pending, out.jobs_running, out.jobs_quarantined) =
            self.person_job_counts(person_id).await?;
        Ok(out)
    }
}

fn unit_hits(memories: Vec<Memory>) -> Vec<Hit> {
    memories
        .into_iter()
        .map(|memory| Hit { memory, score: 1.0 })
        .collect()
}

/// Why-changed order: a decision or a memory that replaced or contradicts another, then one with
/// a stated reason, then one with more links.
fn change_class(m: &Memory) -> (bool, bool, usize) {
    use cairn_core::memory::Relation;
    let changes = m.kind == Kind::Decision
        || m.links
            .iter()
            .any(|l| matches!(l.relation, Relation::Supersedes | Relation::Contradicts));
    (changes, !m.reason.is_empty(), m.links.len())
}

/// Entity names a query can match: its paths and code spans, every word as typed and
/// lower-cased, and the context's files, symbols and commands.
fn query_entity_names(query: &str, context: Option<&WorkContext>) -> Vec<String> {
    let work = context.cloned().unwrap_or_default();
    let mut names: Vec<String> = entities::derive(&[query], &work)
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.extend(tokenize(query));
    names.extend(
        query
            .split_whitespace()
            .map(|w| {
                w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
                    .to_owned()
            })
            .filter(|w| w.len() >= 2),
    );
    names.push(query.trim().to_lowercase());
    names.sort();
    names.dedup();
    names
}
