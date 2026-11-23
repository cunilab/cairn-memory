use crate::capture::{RECENT_FILE_MAX_BYTES, RECENT_FILES_MAX, clip};
use crate::embed::vector_text;
use crate::memory::{MEMORY_COLUMNS, MEMORY_SELECT, memory_from_row, searchable, visible};
use crate::{Result, Store, StoreError};
use cairn_core::capture::WorkContext;
use cairn_core::memory::{Fields, Kind, Memory, Status};
use cairn_core::process::embed::validate_vector;
use cairn_core::process::entities;
use cairn_core::process::entities::module_of;
use cairn_core::retrieve::brief::{session_brief, topic_card};
use cairn_core::retrieve::filter::{Filter, matches};
use cairn_core::retrieve::gate::{self, GateInput};
use cairn_core::retrieve::graph::{Graph, GraphFilter, build, node_cap};
use cairn_core::retrieve::hybrid::{CANDIDATES_PER_CHANNEL, POOL, fuse};
use cairn_core::retrieve::pack::{Cards, Hit, pack, sort_best_first};
use cairn_core::retrieve::rank::adjust;
use serde::Serialize;
use sqlx::postgres::PgArguments;
use sqlx::query::Query;
use sqlx::{Postgres, Row};
use std::collections::{HashMap, HashSet};

/// Fused hits whose neighbours are added to the candidate pool.
const NEIGHBOUR_SEEDS: usize = 32;
/// Rules read for the brief: those on a recent file or its directory, then the newest of any.
const BRIEF_NEAR_RULES: i32 = 100;
const BRIEF_NEWEST_RULES: i32 = 200;

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
    pub relevance: f32,
    pub cosine: Option<f32>,
    pub coverage: f32,
    pub entity_hit: bool,
    pub kept: bool,
}

/// Most rows one lexical leg reads before ranking (see `lexical_sql`).
pub const LEXICAL_MATCH_CAP: i64 = 2000;

/// The request's project and kind, applied inside every channel's `WHERE` so the candidate pool
/// holds only memories the filter would keep. `cairn_core::retrieve::filter::matches` still runs
/// afterwards and is exact (it also reads a handoff's session, branch and worktree).
#[derive(Clone, Debug, Default)]
pub struct Narrow {
    /// A context or session was given: project memories must belong to `project`.
    pub scoped: bool,
    pub project: Option<String>,
    pub kind: Option<String>,
}

impl Narrow {
    pub const NONE: Narrow = Narrow {
        scoped: false,
        project: None,
        kind: None,
    };

    /// ` AND ...` over `alias`, numbering its binds from `$first`; empty when nothing narrows.
    fn sql(&self, alias: &str, first: usize) -> String {
        let mut next = first;
        let mut out = String::new();
        if self.scoped {
            out +=
                &format!(" AND ({alias}.scope <> 'project' OR {alias}.project_id = ${next}::text)");
            next += 1;
        }
        if self.kind.is_some() {
            out += &format!(" AND {alias}.kind = ${next}::text");
        }
        out
    }

    fn bind<'q>(
        &'q self,
        mut q: Query<'q, Postgres, PgArguments>,
    ) -> Query<'q, Postgres, PgArguments> {
        if self.scoped {
            q = q.bind(self.project.as_deref());
        }
        if let Some(kind) = &self.kind {
            q = q.bind(kind.as_str());
        }
        q
    }
}

impl From<&Filter> for Narrow {
    fn from(f: &Filter) -> Self {
        Self {
            scoped: f.session_id.is_some() || f.context.is_some(),
            project: f.context.as_ref().and_then(|c| c.project_id.clone()),
            kind: f.kind.map(|k| k.as_str().to_owned()),
        }
    }
}

/// The fused candidates of one query, loaded with the gate inputs.
struct Pool {
    fused: Vec<(String, f32)>,
    entity_hits: HashSet<String>,
    by_id: HashMap<String, Candidate>,
}

/// One note the hook may inject when a file is first edited.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TopicNote {
    pub id: String,
    pub title: String,
    pub summary: String,
}

/// The reply to `/session/topic`: notes for one or two memories, or a single card for more.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Topic {
    pub notes: Vec<TopicNote>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card: Option<String>,
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
        narrow: &Narrow,
    ) -> Result<Vec<(String, f32)>> {
        scored(
            narrow
                .bind(
                    sqlx::query(&lexical_sql(narrow))
                        .bind(person_id)
                        .bind(query)
                        .bind(projects)
                        .bind(limit),
                )
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
        narrow: &Narrow,
    ) -> Result<Vec<(String, f32)>> {
        scored(
            narrow
                .bind(
                    sqlx::query(&format!(
                        "SELECT e.memory_id, count(*)::float4 AS score
                           FROM entities e JOIN memories m ON m.memory_id = e.memory_id
                          WHERE e.name = ANY($2) AND {}{}
                          GROUP BY e.memory_id, m.created_at
                          ORDER BY score DESC, m.created_at DESC, e.memory_id LIMIT $4",
                        searchable("m", "$1", "$3"),
                        narrow.sql("m", 5)
                    ))
                    .bind(person_id)
                    .bind(names)
                    .bind(projects)
                    .bind(limit),
                )
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
        narrow: &Narrow,
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
        let rows = narrow
            .bind(
                sqlx::query(&vector_sql(narrow))
                    .bind(person_id)
                    .bind(vector_text(vector))
                    .bind(projects)
                    .bind(limit),
            )
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        scored(rows)
    }
}

/// The lexical channel's statement; the plan test explains the same text. Two legs find the
/// matching rows, each capped at `LEXICAL_MATCH_CAP` before anything is ranked: the all-terms
/// leg (so a full match is never crowded out) and the any-term leg (partial matches). A word
/// found in most rows would otherwise make every row be ranked.
// shortcut: a capped leg keeps the first rows the bitmap yields (physical order), not the best;
// upgrade by ranking per rare lexeme if a corpus has >2,000 matches for its rarest term.
pub fn lexical_sql(narrow: &Narrow) -> String {
    let scope = searchable("m", "$1", "$3");
    let narrowed = narrow.sql("m", 5);
    format!(
        "WITH q AS (
           SELECT w AS all_terms,
                  CASE WHEN strpos(w::text, '!') > 0 THEN w
                       ELSE replace(w::text, ' & ', ' | ')::tsquery END AS any_term
             FROM websearch_to_tsquery('english', $2) AS w),
         hit AS MATERIALIZED (
           (SELECT m.memory_id FROM memories m, q
             WHERE m.search_tsv @@ q.all_terms AND {scope}{narrowed} LIMIT {LEXICAL_MATCH_CAP})
           UNION
           (SELECT m.memory_id FROM memories m, q
             WHERE m.search_tsv @@ q.any_term AND {scope}{narrowed} LIMIT {LEXICAL_MATCH_CAP}))
         SELECT m.memory_id, ts_rank_cd(m.search_tsv, q.any_term, 32)::float4 AS score
           FROM hit JOIN memories m USING (memory_id), q
          ORDER BY score DESC, m.created_at DESC, m.memory_id LIMIT $4"
    )
}

/// Aliases are searched only when the 0.1.9 bench clears the ship rule. It did not (v2 weak 9/14,
/// not 14/14), so alias rows are stored and embedded but ignored by search. See `bench/aliases-0.1.9.md`.
pub const ALIASES_IN_SEARCH: bool = false;

/// The vector channel's statement; the plan test explains the same text.
pub fn vector_sql(narrow: &Narrow) -> String {
    let scope = searchable("m", "$1", "$3");
    let narrowed = narrow.sql("m", 5);
    let memory = format!(
        "SELECT m.memory_id, m.created_at, m.embedding <=> $2::vector AS dist
           FROM memories m
          WHERE m.embedding IS NOT NULL AND {scope}{narrowed}
          ORDER BY m.embedding <=> $2::vector
          LIMIT $4"
    );
    if !ALIASES_IN_SEARCH {
        return format!(
            "WITH c AS MATERIALIZED ({memory})
             SELECT memory_id, (1 - dist)::float4 AS score FROM c ORDER BY dist, created_at DESC, memory_id"
        );
    }
    format!(
        "WITH mem AS MATERIALIZED ({memory}),
              als AS MATERIALIZED (
                SELECT m.memory_id, m.created_at, a.embedding <=> $2::vector AS dist
                  FROM memory_aliases a
                  JOIN memories m ON m.memory_id = a.memory_id
                 WHERE a.embedding IS NOT NULL AND {scope}{narrowed}
                 ORDER BY a.embedding <=> $2::vector
                 LIMIT $4),
              best AS (
                SELECT memory_id, min(created_at) AS created_at, min(dist) AS dist
                  FROM (SELECT memory_id, created_at, dist FROM mem
                        UNION ALL
                        SELECT memory_id, created_at, dist FROM als) u
                 GROUP BY memory_id)
         SELECT memory_id, (1 - dist)::float4 AS score
           FROM best
          ORDER BY dist, created_at DESC, memory_id
          LIMIT $4"
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
    /// scored `relevance * (1 + adjust)` and packed under the diversity cap, `budget` tokens and `limit`.
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
        let mut hits = rank(&pool, filter, cairn_core::id::now_seconds());
        if filter.kind == Some(Kind::Handoff) {
            hits = one_handoff_per_checkout(hits);
        }
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
                    relevance: gate::relevance(&input),
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
        let narrow = Narrow::from(filter);
        let lexical = self
            .lexical_candidates(person_id, &projects, query, per_channel, &narrow)
            .await?;
        // Only names the query itself contains can pass the gate; the open files, symbols and
        // commands of the work context are one more ranked channel (closeness), never a pass.
        let names = entity_names(&[query], &WorkContext::default());
        let entity = if names.is_empty() {
            Vec::new()
        } else {
            self.entity_candidates(person_id, &projects, &names, per_channel, &narrow)
                .await?
        };
        let mut context_names = filter
            .context
            .as_ref()
            .map_or_else(Vec::new, |work| entity_names(&[], work));
        context_names.retain(|name| !names.contains(name));
        let near = if context_names.is_empty() {
            Vec::new()
        } else {
            self.entity_candidates(person_id, &projects, &context_names, per_channel, &narrow)
                .await?
        };
        let semantic = match vector {
            Some(vector) => {
                self.vector_candidates(person_id, &projects, vector, per_channel, &narrow)
                    .await?
            }
            None => Vec::new(),
        };
        let entity_hits: HashSet<String> = entity.iter().map(|(id, _)| id.clone()).collect();
        let seeds = fuse(vec![
            lexical.clone(),
            entity.clone(),
            near.clone(),
            semantic.clone(),
        ]);
        let neighbours = self
            .neighbours(person_id, &projects, &seeds, &narrow)
            .await?;
        let mut fused = fuse(vec![lexical, entity, near, semantic, neighbours]);
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
        // The gate sees the best of the memory's own vector and its aliases' (`least` skips NULLs).
        let nearest = if ALIASES_IN_SEARCH {
            "least(m.embedding <=> $4::vector,
                   (SELECT min(a.embedding <=> $4::vector) FROM memory_aliases a
                     WHERE a.memory_id = m.memory_id AND a.embedding IS NOT NULL))"
        } else {
            "(m.embedding <=> $4::vector)"
        };
        let cosine =
            format!("CASE WHEN $4::vector IS NULL THEN NULL ELSE (1 - {nearest})::float4 END");
        let rows = sqlx::query(&format!(
            "WITH q AS (SELECT coalesce(array_agg(DISTINCT lexeme), '{{}}') AS lx
                          FROM unnest(to_tsvector('english', $3)))
             {MEMORY_COLUMNS},
                    {cosine} AS cosine,
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
        narrow: &Narrow,
    ) -> Result<Vec<(String, f32)>> {
        if seeds.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<String> = seeds
            .iter()
            .take(NEIGHBOUR_SEEDS)
            .map(|(id, _)| id.clone())
            .collect();
        let rows = narrow
            .bind(
                sqlx::query(&format!(
                    "SELECT l.source_id, l.target_id FROM links l
                       JOIN memories n ON n.memory_id = CASE WHEN l.source_id = ANY($2) THEN l.target_id ELSE l.source_id END
                      WHERE (l.source_id = ANY($2) OR l.target_id = ANY($2)) AND {}{}
                      ORDER BY n.created_at DESC, n.memory_id, l.source_id, l.target_id",
                    searchable("n", "$1", "$3"),
                    narrow.sql("n", 4)
                ))
                .bind(person_id)
                .bind(&ids)
                .bind(projects),
            )
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
        // First-seen order (newest neighbour first, not link creation time, which depends on job
        // timing), so equal sums keep a stable rank.
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
    /// person's preferences. Nothing is counted as fetched. Each section has its own query and
    /// limit, so a project with hundreds of newer rules still shows its handoff and preferences,
    /// and the rules that name a recent file (or a file beside one) are read before the newest.
    pub async fn brief(
        &self,
        person_id: &str,
        context: &WorkContext,
        recent_files: &[String],
    ) -> Result<String> {
        let recent: Vec<String> = recent_files
            .iter()
            .take(RECENT_FILES_MAX)
            .map(|f| clip(f, RECENT_FILE_MAX_BYTES))
            .collect();
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
        let newest = "ORDER BY m.created_at DESC, m.memory_id";
        let handoff = sqlx::query(&format!(
            "{MEMORY_SELECT}
              WHERE {} AND m.status <> 'forgotten' AND m.rank <> 'candidate' AND m.kind = 'handoff' AND m.owner_id = $1
                AND m.repo = $3 AND m.repo_branch IS NOT DISTINCT FROM $4 AND m.repo_worktree IS NOT DISTINCT FROM $5
                AND m.project_id IS NOT DISTINCT FROM $2
              {newest} LIMIT 50",
            visible("m", "$1")
        ))
        .bind(person_id)
        .bind(&context.project_id)
        .bind(&context.repository)
        .bind(&context.branch)
        .bind(&context.worktree)
        .fetch_all(&self.pool)
        .await?;
        let preferences = sqlx::query(&format!(
            "{MEMORY_SELECT}
              WHERE {} AND m.status = 'active' AND m.rank <> 'candidate' AND m.kind = 'preference' AND m.scope = 'personal'
                AND m.owner_id = $1
              {newest} LIMIT 2",
            visible("m", "$1")
        ))
        .bind(person_id)
        .fetch_all(&self.pool)
        .await?;
        // Project items on this branch or the default branch (`NULL` branch: any), the same rule
        // `session_brief` applies, so the limit never fills with rows it would drop.
        let project_items = |extra: &str, order: &str, limit: i32| {
            format!(
                "{MEMORY_SELECT}
                  WHERE {} AND m.status = 'active' AND m.rank <> 'candidate' AND m.scope = 'project' AND m.project_id = $2
                    AND m.kind IN ('rule','decision','gotcha')
                    AND (m.repo_branch IS NULL OR m.repo_branch = $3
                         OR CASE WHEN $4::text IS NULL THEN m.repo_branch IN ('main','master')
                                 ELSE m.repo_branch = $4 END){extra}
                  {order} LIMIT {limit}",
                visible("m", "$1")
            )
        };
        let mut rows = Vec::new();
        if context.project_id.is_some() {
            if !recent.is_empty() {
                let dirs: Vec<String> = recent
                    .iter()
                    .filter_map(|f| f.rsplit_once('/').map(|(dir, _)| format!("{dir}/")))
                    .collect();
                rows.extend(
                    sqlx::query(&project_items(
                        " AND EXISTS (SELECT 1 FROM entities e
                                       WHERE e.memory_id = m.memory_id AND e.kind = 'file'
                                         AND (e.name = ANY($5)
                                              OR EXISTS (SELECT 1 FROM unnest($6::text[]) d
                                                          WHERE starts_with(e.name, d))))",
                        "ORDER BY m.updated_at DESC, m.memory_id DESC",
                        BRIEF_NEAR_RULES,
                    ))
                    .bind(person_id)
                    .bind(&context.project_id)
                    .bind(&context.branch)
                    .bind(&default_branch)
                    .bind(&recent)
                    .bind(&dirs)
                    .fetch_all(&self.pool)
                    .await?,
                );
            }
            rows.extend(
                sqlx::query(&project_items(
                    "",
                    "ORDER BY m.updated_at DESC, m.memory_id DESC",
                    BRIEF_NEWEST_RULES,
                ))
                .bind(person_id)
                .bind(&context.project_id)
                .bind(&context.branch)
                .bind(&default_branch)
                .fetch_all(&self.pool)
                .await?,
            );
        }
        rows.extend(preferences);
        let mut seen = HashSet::new();
        let mut memories = Vec::new();
        for row in &rows {
            let memory = memory_from_row(row)?;
            if seen.insert(memory.id.clone()) {
                memories.push(memory);
            }
        }
        let mut handoff_rows = Vec::new();
        for row in &handoff {
            handoff_rows.push(memory_from_row(row)?);
        }
        if let Some(mut chosen) = preferred_handoff(&handoff_rows).cloned() {
            // shortcut: session_brief only renders an active handoff; the stored row stays as it is
            chosen.status = Status::Active;
            if seen.insert(chosen.id.clone()) {
                memories.push(chosen);
            }
        }
        self.attach(person_id, &mut memories).await?;
        Ok(session_brief(
            &memories,
            context,
            &recent,
            default_branch.as_deref(),
            cairn_core::id::now_seconds(),
        ))
    }

    /// The active, visible memories about `path`: those with a file entity exactly `path` first, then
    /// those with the module entity of its folder; the caller's project first. Up to two come back
    /// as notes; three or more as one card naming the first two. Handoffs are never topic notes
    /// and nothing is counted as fetched.
    pub async fn topic(&self, person_id: &str, path: &str, context: &WorkContext) -> Result<Topic> {
        let rows = sqlx::query(&format!(
            "SELECT m.memory_id, m.kind, m.title, m.summary, count(*) OVER () AS total
               FROM memories m
               JOIN entities e ON e.memory_id = m.memory_id
              WHERE {} AND m.status = 'active' AND m.kind <> 'handoff'
                AND ((e.kind = 'file' AND e.name = $2) OR (e.kind = 'module' AND e.name = $4))
              GROUP BY m.memory_id
              ORDER BY bool_or(e.kind = 'file') DESC,
                       (m.project_id IS NOT NULL AND m.project_id = $3) DESC,
                       m.updated_at DESC, m.memory_id
              LIMIT 2",
            visible("m", "$1")
        ))
        .bind(person_id)
        .bind(path)
        .bind(&context.project_id)
        .bind(module_of(path))
        .fetch_all(&self.pool)
        .await?;
        let total = match rows.first() {
            Some(row) => row.try_get::<i64, _>("total")?,
            None => 0,
        };
        if total >= 3 {
            let kinds = rows
                .iter()
                .map(|r| {
                    r.try_get::<String, _>("kind")?
                        .parse::<Kind>()
                        .map_err(StoreError::Validation)
                })
                .collect::<Result<Vec<_>>>()?;
            let titles = rows
                .iter()
                .map(|r| r.try_get::<String, _>("title"))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let first: Vec<(Kind, &str)> = kinds
                .iter()
                .copied()
                .zip(titles.iter().map(String::as_str))
                .collect();
            return Ok(Topic {
                notes: Vec::new(),
                card: Some(topic_card(path, total as usize, &first)),
            });
        }
        let notes = rows
            .iter()
            .map(|r| {
                Ok(TopicNote {
                    id: r.try_get("memory_id")?,
                    title: r.try_get("title")?,
                    summary: r.try_get("summary")?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Topic { notes, card: None })
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
}

fn handoff_is_thin(memory: &Memory) -> bool {
    match &memory.fields {
        Fields::Handoff(handoff) => handoff.files.is_empty() && handoff.open_questions.is_empty(),
        _ => false,
    }
}

fn newest_first(a: &&Memory, b: &&Memory) -> std::cmp::Ordering {
    a.created_us.cmp(&b.created_us).then(b.id.cmp(&a.id))
}

/// Newest handoff of the slice. When that one is thin, the newest that is not.
fn preferred_handoff(memories: &[Memory]) -> Option<&Memory> {
    let newest = memories.iter().max_by(newest_first)?;
    if !handoff_is_thin(newest) {
        return Some(newest);
    }
    memories
        .iter()
        .filter(|memory| !handoff_is_thin(memory))
        .max_by(newest_first)
        .or(Some(newest))
}

fn checkout_of(memory: &Memory) -> (String, String, String, String, String) {
    (
        memory.owner.clone(),
        memory.project_id.clone().unwrap_or_default(),
        memory.context.repo.clone().unwrap_or_default(),
        memory.context.branch.clone().unwrap_or_default(),
        memory.context.worktree.clone().unwrap_or_default(),
    )
}

/// `kind=handoff` returns one card per checkout: the newest, or the newest non-thin one when the
/// newest is thin.
fn one_handoff_per_checkout(hits: Vec<Hit>) -> Vec<Hit> {
    let mut groups: HashMap<(String, String, String, String, String), Vec<Hit>> = HashMap::new();
    let mut order = Vec::new();
    for hit in hits {
        let key = checkout_of(&hit.memory);
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(hit);
    }
    let mut out = Vec::new();
    for key in order {
        let group = groups.remove(&key).unwrap_or_default();
        let memories: Vec<Memory> = group.iter().map(|hit| hit.memory.clone()).collect();
        let Some(chosen) = preferred_handoff(&memories) else {
            continue;
        };
        if let Some(hit) = group.into_iter().find(|hit| hit.memory.id == chosen.id) {
            out.push(hit);
        }
    }
    sort_best_first(&mut out);
    out
}

fn gate_input(pool: &Pool, id: &str, c: &Candidate) -> GateInput {
    GateInput {
        entity_hit: pool.entity_hits.contains(id),
        coverage: c.coverage,
        cosine: c.cosine,
    }
}

/// The hits that pass the request filter and the gate, best first: score descending, then newest,
/// then id. The score is the absolute relevance (`gate::relevance`) times `1 + adjust`, so it says
/// how good the match is, not where it ranks among the others. The fused rank only chose which
/// candidates are loaded.
fn rank(pool: &Pool, filter: &Filter, now: i64) -> Vec<Hit> {
    let ctx = filter.context.clone().unwrap_or_default();
    let mut hits: Vec<Hit> = pool
        .fused
        .iter()
        .filter_map(|(id, _)| {
            let c = pool.by_id.get(id)?;
            let input = gate_input(pool, id, c);
            let eligible = matches(&c.memory, filter) && gate::keep(&input);
            eligible.then(|| Hit {
                score: gate::relevance(&input) * (1.0 + adjust(&c.memory, &ctx, now)),
                memory: c.memory.clone(),
            })
        })
        .collect();
    sort_best_first(&mut hits);
    hits
}

/// Entity names in `texts` (paths and code spans) and in the work context's files, symbols,
/// commands, tests and services. Plain words are not entity names; the lexical channel has them.
fn entity_names(texts: &[&str], work: &WorkContext) -> Vec<String> {
    let mut names: Vec<String> = entities::derive(texts, work)
        .into_iter()
        .map(|e| e.name)
        .collect();
    names.sort();
    names.dedup();
    names
}
