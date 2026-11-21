use crate::embed::vector_text;
use crate::memory::{MEMORY_COLUMNS, MEMORY_SELECT, memory_from_row, searchable, visible};
use crate::{Result, Store};
use cairn_core::capture::WorkContext;
use cairn_core::memory::Memory;
use cairn_core::process::embed::validate_vector;
use cairn_core::process::entities;
use cairn_core::retrieve::brief::session_brief;
use cairn_core::retrieve::filter::{Filter, matches};
use cairn_core::retrieve::gate::{self, GateInput};
use cairn_core::retrieve::graph::{Graph, GraphFilter, build, node_cap};
use cairn_core::retrieve::hybrid::{CANDIDATES_PER_CHANNEL, POOL, fuse};
use cairn_core::retrieve::pack::{Cards, Hit, pack};
use cairn_core::retrieve::rank::adjust;
use serde::Serialize;
use sqlx::Row;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Fused hits whose neighbours are added to the candidate pool.
const NEIGHBOUR_SEEDS: usize = 32;

/// A candidate with the inputs of the relevance gate.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub memory: Memory,
    /// Cosine to the query vector; `None` without a query vector or a stored embedding.
    pub cosine: Option<f32>,
    /// Share of the query's lexemes the memory's text contains, 0.0..=1.0.
    pub coverage: f32,
}

/// One fused candidate before the gate: the numbers calibration reads (`bench.py --dump-scores`).
#[derive(Clone, Debug, Serialize)]
pub struct Trace {
    pub id: String,
    pub rrf: f32,
    pub cosine: Option<f32>,
    pub coverage: f32,
    pub entity_hit: bool,
    pub kept: bool,
}

/// The fused candidates of one query, loaded with the gate inputs.
struct Pool {
    fused: Vec<(String, f32)>,
    entity_hits: HashSet<String>,
    by_id: HashMap<String, Candidate>,
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

/// One note the hook may inject when a file is first edited.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TopicNote {
    pub id: String,
    pub title: String,
    pub summary: String,
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

    /// The project ids the person belongs to: the `$P` of every search predicate.
    pub async fn member_projects(&self, person_id: &str) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT project_id FROM project_members WHERE person_id = $1 ORDER BY project_id",
        )
        .bind(person_id)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Full-text channel (`memories_search_idx`): `websearch_to_tsquery('english', q)` with `&`
    /// read as `|` unless the query negates a word, so a partial match is a candidate; the gate
    /// measures its coverage. Score is `ts_rank_cd`.
    pub async fn lexical_candidates(
        &self,
        person_id: &str,
        projects: &[String],
        query: &str,
        limit: i64,
    ) -> Result<Vec<(String, f32)>> {
        scored(
            sqlx::query(&lexical_sql())
                .bind(person_id)
                .bind(query)
                .bind(projects)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?,
        )
    }

    /// Entity channel: score is the number of matching entity names, via `entities_lookup_idx`.
    pub async fn entity_candidates(
        &self,
        person_id: &str,
        projects: &[String],
        names: &[String],
        limit: i64,
    ) -> Result<Vec<(String, f32)>> {
        scored(
            sqlx::query(&format!(
                "SELECT e.memory_id, count(*)::float4 AS score
                   FROM entities e JOIN memories m ON m.memory_id = e.memory_id
                  WHERE e.name = ANY($2) AND {}
                  GROUP BY e.memory_id, m.created_at
                  ORDER BY score DESC, m.created_at DESC, e.memory_id LIMIT $4",
                searchable("m", "$1", "$3")
            ))
            .bind(person_id)
            .bind(names)
            .bind(projects)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?,
        )
    }

    /// Semantic channel: the nearest vectors by cosine through `memories_embedding_hnsw`, with the
    /// search rule in the same `WHERE`. pgvector's iterative index scan keeps reading the graph
    /// until `limit` rows pass it (or `max_scan_tuples`), so a person who owns 2 % of the rows
    /// still gets `limit` hits. `relaxed_order` may return near-ties out of order; the outer
    /// `ORDER BY` restores the exact order. Score is cosine similarity.
    pub async fn vector_candidates(
        &self,
        person_id: &str,
        projects: &[String],
        vector: &[f32],
        limit: i64,
    ) -> Result<Vec<(String, f32)>> {
        validate_vector(vector)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "SELECT set_config('hnsw.iterative_scan', 'relaxed_order', true),
                    set_config('hnsw.ef_search', '100', true),
                    set_config('hnsw.max_scan_tuples', '20000', true)",
        )
        .execute(&mut *tx)
        .await?;
        let rows = sqlx::query(&vector_sql())
            .bind(person_id)
            .bind(vector_text(vector))
            .bind(projects)
            .bind(limit)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        scored(rows)
    }
}

/// The lexical channel's statement; the plan test explains the same text.
pub fn lexical_sql() -> String {
    format!(
        "WITH q AS (
           SELECT CASE WHEN strpos(w::text, '!') > 0 THEN w
                       ELSE replace(w::text, ' & ', ' | ')::tsquery END AS query
             FROM websearch_to_tsquery('english', $2) AS w)
         SELECT m.memory_id, ts_rank_cd(m.search_tsv, q.query, 32)::float4 AS score
           FROM memories m, q
          WHERE m.search_tsv @@ q.query AND {}
          ORDER BY score DESC, m.created_at DESC, m.memory_id LIMIT $4",
        searchable("m", "$1", "$3")
    )
}

/// The vector channel's statement; the plan test explains the same text.
pub fn vector_sql() -> String {
    format!(
        "WITH c AS MATERIALIZED (
           SELECT m.memory_id, m.created_at, m.embedding <=> $2::vector AS dist
             FROM memories m
            WHERE m.embedding IS NOT NULL AND {}
            ORDER BY m.embedding <=> $2::vector
            LIMIT $4)
         SELECT memory_id, (1 - dist)::float4 AS score FROM c ORDER BY dist, created_at DESC, memory_id",
        searchable("m", "$1", "$3")
    )
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
    /// the search rule (`searchable`), so no hit can come from another person's private memory;
    /// the vector channel applies it inside the index scan. Hits pass the relevance gate, are
    /// scored `RRF * (1 + adjust)` and packed under the diversity cap, `budget` tokens and `limit`.
    /// An agent read counts as a fetch; nothing else does.
    pub async fn search(
        &self,
        person_id: &str,
        query: &str,
        vector: Option<&[f32]>,
        limit: usize,
        budget: usize,
        filter: &Filter,
    ) -> Result<Cards> {
        let pool = self
            .candidate_pool(person_id, query, vector, filter)
            .await?;
        let hits = rank(&pool, filter, cairn_core::id::now_seconds());
        let cards = pack(&hits, limit, budget);
        let ids: Vec<String> = cards.results.iter().map(|c| c.id.clone()).collect();
        self.mark_fetched(&ids).await?;
        Ok(cards)
    }

    /// Every fused candidate with its gate inputs and whether the gate keeps it, for calibration.
    pub async fn score_trace(
        &self,
        person_id: &str,
        query: &str,
        vector: Option<&[f32]>,
        filter: &Filter,
    ) -> Result<Vec<Trace>> {
        let pool = self
            .candidate_pool(person_id, query, vector, filter)
            .await?;
        Ok(pool
            .fused
            .iter()
            .filter_map(|(id, rrf)| {
                let c = pool.by_id.get(id)?;
                let input = gate_input(&pool, id, c);
                Some(Trace {
                    id: id.clone(),
                    rrf: *rrf,
                    cosine: c.cosine,
                    coverage: c.coverage,
                    entity_hit: input.entity_hit,
                    kept: gate::keep(&input),
                })
            })
            .collect())
    }

    async fn candidate_pool(
        &self,
        person_id: &str,
        query: &str,
        vector: Option<&[f32]>,
        filter: &Filter,
    ) -> Result<Pool> {
        if query.trim().is_empty() {
            return Err(crate::StoreError::Validation(
                "query must not be empty".into(),
            ));
        }
        let projects = self.member_projects(person_id).await?;
        let per_channel = CANDIDATES_PER_CHANNEL as i64;
        let lexical = self
            .lexical_candidates(person_id, &projects, query, per_channel)
            .await?;
        let names = query_entity_names(query, filter.context.as_ref());
        let entity = if names.is_empty() {
            Vec::new()
        } else {
            self.entity_candidates(person_id, &projects, &names, per_channel)
                .await?
        };
        let semantic = match vector {
            Some(vector) => {
                self.vector_candidates(person_id, &projects, vector, per_channel)
                    .await?
            }
            None => Vec::new(),
        };
        let entity_hits: HashSet<String> = entity.iter().map(|(id, _)| id.clone()).collect();
        let seeds = fuse(vec![lexical.clone(), entity.clone(), semantic.clone()]);
        let neighbours = self.neighbours(person_id, &projects, &seeds).await?;
        let mut fused = fuse(vec![lexical, entity, semantic, neighbours]);
        fused.truncate(POOL);
        let ids: Vec<String> = fused.iter().map(|(id, _)| id.clone()).collect();
        let by_id = self
            .load_candidates(person_id, &projects, &ids, query, vector)
            .await?
            .into_iter()
            .map(|c| (c.memory.id.clone(), c))
            .collect();
        Ok(Pool {
            fused,
            entity_hits,
            by_id,
        })
    }

    /// The fused candidates as full memories, each with the relevance-gate inputs: the cosine to
    /// the query vector (`None` without a vector or an embedding) and the share of the query's
    /// lexemes the memory's text contains. Unknown and unsearchable ids are absent.
    pub async fn load_candidates(
        &self,
        person_id: &str,
        projects: &[String],
        ids: &[String],
        query: &str,
        vector: Option<&[f32]>,
    ) -> Result<Vec<Candidate>> {
        let rows = sqlx::query(&format!(
            "WITH q AS (SELECT coalesce(array_agg(DISTINCT lexeme), '{{}}') AS lx
                          FROM unnest(to_tsvector('english', $3)))
             {MEMORY_COLUMNS},
                    CASE WHEN $4::vector IS NULL OR m.embedding IS NULL THEN NULL
                         ELSE (1 - (m.embedding <=> $4::vector))::float4 END AS cosine,
                    CASE WHEN cardinality(q.lx) = 0 THEN 0::float4 ELSE
                      ((SELECT count(*) FROM unnest(m.search_tsv) t WHERE t.lexeme = ANY(q.lx))::float4
                        / cardinality(q.lx))::float4 END AS coverage
               FROM memories m LEFT JOIN usage u ON u.memory_id = m.memory_id, q
              WHERE m.memory_id = ANY($2) AND {}",
            searchable("m", "$1", "$5")
        ))
        .bind(person_id)
        .bind(ids)
        .bind(query)
        .bind(vector.map(vector_text))
        .bind(projects)
        .fetch_all(&self.pool)
        .await?;
        let mut memories = rows
            .iter()
            .map(memory_from_row)
            .collect::<Result<Vec<_>>>()?;
        self.attach(person_id, &mut memories).await?;
        rows.iter()
            .zip(memories)
            .map(|(row, memory)| {
                Ok(Candidate {
                    memory,
                    cosine: row.try_get("cosine")?,
                    coverage: row.try_get("coverage")?,
                })
            })
            .collect()
    }

    /// One hop from the fused seeds over `links`: each linked memory the person can search gets
    /// the summed score of its seeds, as a ranked channel.
    pub async fn neighbours(
        &self,
        person_id: &str,
        projects: &[String],
        seeds: &[(String, f32)],
    ) -> Result<Vec<(String, f32)>> {
        if seeds.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<String> = seeds
            .iter()
            .take(NEIGHBOUR_SEEDS)
            .map(|(id, _)| id.clone())
            .collect();
        let rows = sqlx::query(&format!(
            "SELECT l.source_id, l.target_id FROM links l
               JOIN memories n ON n.memory_id = CASE WHEN l.source_id = ANY($2) THEN l.target_id ELSE l.source_id END
              WHERE (l.source_id = ANY($2) OR l.target_id = ANY($2)) AND {}
              ORDER BY l.created_at, l.source_id, l.target_id",
            searchable("n", "$1", "$3")
        ))
        .bind(person_id)
        .bind(&ids)
        .bind(projects)
        .fetch_all(&self.pool)
        .await?;
        let score: HashMap<&str, f32> = seeds.iter().map(|(id, s)| (id.as_str(), *s)).collect();
        let mut sum: HashMap<String, f32> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for row in &rows {
            let (source, target): (String, String) =
                (row.try_get("source_id")?, row.try_get("target_id")?);
            for (seed, other) in [(&source, &target), (&target, &source)] {
                if let Some(s) = score.get(seed.as_str())
                    && ids.contains(seed)
                {
                    if !sum.contains_key(other) {
                        order.push(other.clone());
                    }
                    *sum.entry(other.clone()).or_default() += s;
                }
            }
        }
        // First-seen order, so equal sums keep a stable rank.
        Ok(order
            .into_iter()
            .filter(|id| !ids.contains(id))
            .map(|id| {
                let total = sum[&id];
                (id, total)
            })
            .collect())
    }

    /// The fenced brief a hook injects at session start: this branch's last handoff, the project's
    /// rules, decisions and gotchas ranked by `recent_files`, branch and feedback, and the
    /// person's preferences. Nothing is counted as fetched.
    pub async fn brief(
        &self,
        person_id: &str,
        context: &WorkContext,
        recent_files: &[String],
    ) -> Result<String> {
        let rows = sqlx::query(&format!(
            "{MEMORY_SELECT}
              WHERE {} AND m.status = 'active'
                AND ((m.kind = 'handoff' AND m.owner_id = $1 AND m.repo = $3
                      AND m.repo_branch = $4 AND m.repo_worktree IS NOT DISTINCT FROM $5
                      AND m.project_id IS NOT DISTINCT FROM $2)
                     OR (m.kind IN ('rule','decision','gotcha') AND m.scope = 'project' AND m.project_id = $2)
                     OR (m.kind = 'preference' AND m.owner_id = $1))
              ORDER BY m.updated_at DESC, m.memory_id DESC LIMIT 300",
            visible("m", "$1")
        ))
        .bind(person_id)
        .bind(&context.project_id)
        .bind(&context.repository)
        .bind(&context.branch)
        .bind(&context.worktree)
        .fetch_all(&self.pool)
        .await?;
        let mut memories = rows
            .iter()
            .map(memory_from_row)
            .collect::<Result<Vec<_>>>()?;
        self.attach(person_id, &mut memories).await?;
        let default_branch: Option<String> = match &context.project_id {
            Some(id) => {
                sqlx::query_scalar("SELECT default_branch FROM projects WHERE project_id = $1")
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?
                    .flatten()
            }
            None => None,
        };
        Ok(session_brief(
            &memories,
            context,
            recent_files,
            default_branch.as_deref(),
            cairn_core::id::now_seconds(),
        ))
    }

    /// Up to two active, visible notes whose file entity is exactly `path`, the caller's project
    /// first. Handoffs are never topic notes and nothing is counted as fetched.
    pub async fn topic_notes(
        &self,
        person_id: &str,
        path: &str,
        context: &WorkContext,
    ) -> Result<Vec<TopicNote>> {
        let rows = sqlx::query(&format!(
            "SELECT m.memory_id, m.title, m.summary FROM memories m
              WHERE {} AND m.status = 'active' AND m.kind <> 'handoff'
                AND EXISTS (SELECT 1 FROM entities e
                             WHERE e.memory_id = m.memory_id AND e.kind = 'file' AND e.name = $2)
              ORDER BY (m.project_id IS NOT NULL AND m.project_id = $3) DESC,
                       m.updated_at DESC, m.memory_id
              LIMIT 2",
            visible("m", "$1")
        ))
        .bind(person_id)
        .bind(path)
        .bind(&context.project_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok(TopicNote {
                    id: r.try_get("memory_id")?,
                    title: r.try_get("title")?,
                    summary: r.try_get("summary")?,
                })
            })
            .collect()
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

fn gate_input(pool: &Pool, id: &str, c: &Candidate) -> GateInput {
    GateInput {
        entity_hit: pool.entity_hits.contains(id),
        coverage: c.coverage,
        cosine: c.cosine,
    }
}

/// The hits that pass the request filter and the gate, best first: score descending, then newest,
/// then id.
fn rank(pool: &Pool, filter: &Filter, now: i64) -> Vec<Hit> {
    let ctx = filter.context.clone().unwrap_or_default();
    let mut hits: Vec<Hit> = pool
        .fused
        .iter()
        .filter_map(|(id, rrf)| {
            let c = pool.by_id.get(id)?;
            let eligible = matches(&c.memory, filter) && gate::keep(&gate_input(pool, id, c));
            eligible.then(|| Hit {
                score: rrf * (1.0 + adjust(&c.memory, &ctx, now)),
                memory: c.memory.clone(),
            })
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.memory.created_us.cmp(&a.memory.created_us))
            .then_with(|| a.memory.id.cmp(&b.memory.id))
    });
    hits
}

/// Entity names a query can match: the paths and code spans it contains, and the work context's
/// files, symbols and commands. Plain words are not entity names; the lexical channel has them.
fn query_entity_names(query: &str, context: Option<&WorkContext>) -> Vec<String> {
    let work = context.cloned().unwrap_or_default();
    let mut names: Vec<String> = entities::derive(&[query], &work)
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    names.dedup();
    names
}
