use crate::embed::vector_text;
use crate::memory::{
    MEMORY_COLUMNS, MEMORY_SELECT, MEMORY_SELECT_WITH_DETAIL, SEARCH_VECTOR, collection_memory,
    memory_from_row, search_matches, searchable, visible,
};
use crate::usage::{Lookup, SESSION_PUSH_LIMIT, already_shown, finish_push, repeat_error, unmuted};
use crate::{Result, Store, StoreError};
use cairn_core::capture::{WorkContext, redact_sensitive_content};
use cairn_core::id::sha256_hex;
use cairn_core::memory::{Fields, Kind, Memory, PROVEN_HELPFUL, PROVEN_SAVES, Status, TrustLevel};
use cairn_core::process::embed::validate_vector;
use cairn_core::process::entities;
use cairn_core::process::entities::module_of;
use cairn_core::retrieve::brief::{
    BriefBudget, SessionBrief, card_age, session_brief, topic_card, topic_notes,
};
use cairn_core::retrieve::command::{error_tail, first_error_line};
use cairn_core::retrieve::filter::{Filter, matches};
use cairn_core::retrieve::gate::{self, GateInput};
use cairn_core::retrieve::graph::{Graph, GraphFilter, build, node_cap};
use cairn_core::retrieve::hybrid::{CANDIDATES_PER_CHANNEL, POOL, fuse};
use cairn_core::retrieve::pack::{Cards, Hit, pack, sort_best_first};
use cairn_core::retrieve::prompt::{matching_word, named_preference};
use cairn_core::retrieve::rank::adjust;
use serde::Serialize;
use sqlx::postgres::PgArguments;
use sqlx::query::Query;
use sqlx::{Postgres, Row};
use std::collections::{HashMap, HashSet};

/// Fused hits whose neighbours are added to the candidate pool.
const NEIGHBOUR_SEEDS: usize = 32;
// The normal gate admits half-coverage and cosine-only hits. A push needs stronger evidence.
const PROMPT_RELEVANCE_MIN: f32 = 0.60;
/// The proven and normal cases of Memory::trust, applied before any SQL limit.
const PUSH_TRUST: &str =
    "m.status <> 'stale' AND m.rank <> 'candidate' AND coalesce(u.wrong, 0) = 0";

/// L1 ranks the normal and proven cases of Memory::trust before applying the card limit.
fn essential_order() -> String {
    format!(
        "ORDER BY (m.proof_count >= {PROVEN_SAVES} OR coalesce(u.helpful, 0) >= {PROVEN_HELPFUL}) DESC,
                  (coalesce(u.fetched, 0)::bigint + coalesce(u.helpful, 0)) DESC,
                  m.updated_at DESC, m.memory_id"
    )
}

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
    prompt_session: Option<String>,
}

impl Narrow {
    pub const NONE: Narrow = Narrow {
        scoped: false,
        project: None,
        kind: None,
        prompt_session: None,
    };

    /// ` AND ...` over `alias`, numbering its binds from `$first`; empty when nothing narrows.
    fn sql(&self, alias: &str, person: &str, first: usize) -> String {
        if self.prompt_session.is_some() {
            let session = first + 1;
            return format!(
                " AND (({alias}.scope = 'personal' AND {alias}.kind IN ('preference','rule'))
                       OR ({alias}.scope = 'project' AND {alias}.project_id = ${first}
                           AND {alias}.kind IN ('rule','decision','gotcha','procedure')))
                  AND {alias}.status = 'active' AND {alias}.rank <> 'candidate'
                  AND NOT EXISTS (SELECT 1 FROM usage u WHERE u.memory_id = {alias}.memory_id AND u.wrong > 0)
                  AND {}
                  AND NOT EXISTS (SELECT 1 FROM session_shown s
                       WHERE s.person_id = {person} AND s.session_id = ${session}
                         AND s.memory_id = {alias}.memory_id)",
                unmuted(alias, person)
            );
        }
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
        if let Some(session) = &self.prompt_session {
            return q.bind(self.project.as_deref()).bind(session);
        }
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
            prompt_session: None,
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
    pub why: String,
    pub age: String,
}

/// The reply to `/session/topic`: notes for one or two memories, or a single card for more.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Topic {
    pub notes: Vec<TopicNote>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card: Option<String>,
    #[serde(skip)]
    pub memory_ids: Vec<String>,
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
        self.attach(person_id, &mut memories, false).await?;
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
                        narrow.sql("m", "$1", 5)
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
    let narrowed = narrow.sql("m", "$1", 5);
    let all = search_matches("q.all_terms");
    let any = search_matches("q.any_term");
    format!(
        "WITH q AS (
           SELECT w AS all_terms,
                  CASE WHEN strpos(w::text, '!') > 0 THEN w
                       ELSE replace(w::text, ' & ', ' | ')::tsquery END AS any_term
             FROM websearch_to_tsquery('english', $2) AS w),
         hit AS MATERIALIZED (
           (SELECT m.memory_id FROM memories m, q
             WHERE {all} AND {scope}{narrowed} LIMIT {LEXICAL_MATCH_CAP})
           UNION
           (SELECT m.memory_id FROM memories m, q
             WHERE {any} AND {scope}{narrowed} LIMIT {LEXICAL_MATCH_CAP}))
         SELECT m.memory_id, ts_rank_cd({SEARCH_VECTOR}, q.any_term, 32)::float4 AS score
           FROM hit JOIN memories m USING (memory_id), q
          ORDER BY score DESC, m.created_at DESC, m.memory_id LIMIT $4"
    )
}

/// Aliases are searched only when the 0.1.9 bench clears the ship rule. It did not (v2 weak 9/14,
/// not 14/14), so alias rows are stored while their embedding and search are paused. See `bench/aliases-0.1.9.md`.
pub const ALIASES_IN_SEARCH: bool = false;

/// The vector channel's statement; the plan test explains the same text.
pub fn vector_sql(narrow: &Narrow) -> String {
    let scope = searchable("m", "$1", "$3");
    let narrowed = narrow.sql("m", "$1", 5);
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
            "{MEMORY_SELECT_WITH_DETAIL} WHERE m.memory_id = ANY($2) AND {}",
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
        self.attach(person_id, &mut memories, false).await?;
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
            .candidate_pool(person_id, query, vector, filter, None)
            .await?;
        let mut hits = rank(&pool, filter, cairn_core::id::now_seconds());
        if filter.kind == Some(Kind::Handoff) {
            hits = one_handoff_per_checkout(hits);
        }
        let cards = pack(&hits, limit, budget);
        let ids: Vec<String> = cards.results.iter().map(|c| c.id.clone()).collect();
        self.mark_fetched(&ids).await?;
        self.record_push_use(person_id, None, &ids, true).await?;
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
            .candidate_pool(person_id, query, vector, filter, None)
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
        prompt_session: Option<&str>,
    ) -> Result<Pool> {
        if query.trim().is_empty() {
            return Err(crate::StoreError::Validation(
                "query must not be empty".into(),
            ));
        }
        let projects = self.member_projects(person_id).await?;
        let per_channel = CANDIDATES_PER_CHANNEL as i64;
        let mut narrow = Narrow::from(filter);
        narrow.prompt_session = prompt_session.map(str::to_owned);
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
            .load_candidates_inner(
                person_id,
                &projects,
                &ids,
                query,
                vector,
                prompt_session.is_none(),
            )
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
        self.load_candidates_inner(person_id, projects, ids, query, vector, true)
            .await
    }

    async fn load_candidates_inner(
        &self,
        person_id: &str,
        projects: &[String],
        ids: &[String],
        query: &str,
        vector: Option<&[f32]>,
        with_attachments: bool,
    ) -> Result<Vec<Candidate>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
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
                      ((SELECT count(*) FROM unnest({SEARCH_VECTOR}) t WHERE t.lexeme = ANY(q.lx))::float4
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
        let (sources, mut memories): (Vec<_>, Vec<_>) = rows
            .iter()
            .filter_map(|row| {
                collection_memory(row)
                    .map(|memory| memory.map(|memory| (row, memory)))
                    .transpose()
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .unzip();
        if with_attachments {
            self.attach(person_id, &mut memories, false).await?;
        }
        sources
            .into_iter()
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
                    narrow.sql("n", "$1", 4)
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

    pub async fn push_budget_spent(&self, person_id: &str, session_id: &str) -> Result<bool> {
        let shown: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM session_shown WHERE person_id = $1 AND session_id = $2",
        )
        .bind(person_id)
        .bind(session_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(shown >= SESSION_PUSH_LIMIT as i64)
    }

    /// Automatic prompt recall shares the hybrid candidates and scoring, without marking a fetch.
    pub async fn prompt_recall(
        &self,
        person_id: &str,
        session_id: &str,
        context: WorkContext,
        prompt: &str,
        vector: Option<&[f32]>,
    ) -> Result<(String, bool)> {
        let filter = Filter {
            context: Some(context.clone()),
            ..Filter::default()
        };
        let pool = self
            .candidate_pool(person_id, prompt, vector, &filter, Some(session_id))
            .await?;
        let now = cairn_core::id::now_seconds();
        let mut ranked = Vec::new();
        for (id, _) in &pool.fused {
            let Some(candidate) = pool.by_id.get(id) else {
                continue;
            };
            let memory = &candidate.memory;
            if memory.status != Status::Active
                || !matches!(memory.trust().0, TrustLevel::Normal | TrustLevel::Proven)
            {
                continue;
            }
            let input = gate_input(&pool, id, candidate);
            let named = (memory.kind == Kind::Preference)
                .then(|| named_preference(prompt, &memory.title, &memory.summary))
                .flatten();
            let relevance = if named.is_some() {
                gate::relevance(&input).max(gate::ENTITY_RELEVANCE)
            } else if gate::keep(&input) {
                gate::relevance(&input)
            } else {
                continue;
            };
            if relevance >= PROMPT_RELEVANCE_MIN {
                let why = named.or_else(|| matching_word(prompt, &memory.title, &memory.summary));
                ranked.push((
                    relevance * (1.0 + adjust(memory, &context, now)),
                    memory,
                    why,
                ));
            }
        }
        if ranked.is_empty() {
            return Ok((String::new(), false));
        }
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0));

        // Rank outside the lock, then recheck the session ledger and mute state before committing.
        let mut tx = self.pool.begin().await?;
        let session = Some(session_id);
        let shown = already_shown(&mut tx, person_id, session).await?;
        let remaining = SESSION_PUSH_LIMIT.saturating_sub(shown.len());
        if remaining == 0 {
            return Ok((String::new(), true));
        }
        let ids: Vec<_> = ranked.iter().map(|(_, memory, _)| &memory.id).collect();
        let mut narrow = Narrow::from(&filter);
        narrow.prompt_session = Some(session_id.to_owned());
        let statement = format!(
            "SELECT m.memory_id FROM memories m WHERE m.memory_id = ANY($2) AND {}{}",
            visible("m", "$1"),
            narrow.sql("m", "$1", 3)
        );
        let eligible: HashSet<String> = sqlx::query_scalar(&statement)
            .bind(person_id)
            .bind(ids)
            .bind(&narrow.project)
            .bind(session_id)
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect();
        let chosen: Vec<_> = ranked
            .into_iter()
            .filter(|(_, memory, _)| eligible.contains(&memory.id))
            .take(2.min(remaining))
            .map(|(_, memory, why)| {
                let why = why.map_or_else(
                    || "matches your prompt".to_owned(),
                    |word| format!("matches \"{word}\" in your prompt"),
                );
                (
                    memory,
                    &memory.title[..memory.title.floor_char_boundary(40)],
                    &memory.summary[..memory.summary.floor_char_boundary(34)],
                    why,
                    card_age(memory.created_at, now),
                )
            })
            .collect();
        let notes: Vec<_> = chosen
            .iter()
            .map(|(_, title, summary, why, age)| (*title, *summary, why.as_str(), age.as_str()))
            .collect();
        let (text, count) = topic_notes("your prompt", &notes);
        let ids: Vec<_> = chosen
            .into_iter()
            .take(count)
            .map(|(memory, ..)| memory.id.clone())
            .collect();
        finish_push(tx, person_id, session, "prompt", &ids).await?;
        Ok((text, count == remaining))
    }

    /// The session brief reserves only the cards rendered in L0 and L1.
    pub async fn brief(
        &self,
        person_id: &str,
        context: &WorkContext,
        session_id: Option<&str>,
        budget: BriefBudget,
    ) -> Result<SessionBrief> {
        let mut tx = self.pool.begin().await?;
        let mut shown = already_shown(&mut tx, person_id, session_id).await?;
        let limit = SESSION_PUSH_LIMIT.saturating_sub(shown.len());
        if limit == 0 {
            return Ok(SessionBrief::default());
        }

        let visible = visible("m", "$1");
        let project = "(m.scope = 'project' AND m.project_id = $2 AND EXISTS (
            SELECT 1 FROM project_members pm WHERE pm.project_id = $2 AND pm.person_id = $1))";
        let personal = "(m.scope = 'personal' AND m.owner_id = $1)";
        let pins = sqlx::query(&format!(
            "{MEMORY_SELECT}
              WHERE {visible} AND ({personal} OR {project})
                AND m.status IN ('active','stale') AND m.pinned_at IS NOT NULL
                AND m.memory_id <> ALL($3)
              ORDER BY m.pinned_at, m.memory_id LIMIT $4"
        ))
        .bind(person_id)
        .bind(&context.project_id)
        .bind(&shown)
        .bind(limit as i64)
        .fetch_all(&mut *tx)
        .await?;
        let pins = pins
            .iter()
            .map(memory_from_row)
            .collect::<Result<Vec<_>>>()?;
        shown.extend(pins.iter().map(|pin| pin.id.clone()));

        let unmuted = unmuted("m", "$1");
        let handoff = if context.repository.is_some() {
            let rows = sqlx::query(&format!(
                "{MEMORY_SELECT}
                  WHERE {visible} AND {PUSH_TRUST} AND {unmuted} AND m.kind = 'handoff' AND m.owner_id = $1
                    AND m.repo = $3 AND m.repo_branch IS NOT DISTINCT FROM $4 AND m.repo_worktree IS NOT DISTINCT FROM $5
                    AND m.project_id IS NOT DISTINCT FROM $2
                  ORDER BY m.created_at DESC, m.memory_id LIMIT 50"
            ))
            .bind(person_id)
            .bind(&context.project_id)
            .bind(&context.repository)
            .bind(&context.branch)
            .bind(&context.worktree)
            .fetch_all(&mut *tx)
            .await?;
            let rows = rows
                .iter()
                .map(memory_from_row)
                .collect::<Result<Vec<_>>>()?;
            preferred_handoff(&rows)
                .filter(|row| row.pinned_at.is_none())
                .map(|row| row.id.clone())
        } else {
            None
        };
        let essentials = sqlx::query(&format!(
            "{MEMORY_SELECT}
              WHERE {visible} AND {PUSH_TRUST} AND {unmuted} AND m.pinned_at IS NULL AND m.memory_id <> ALL($4)
                AND (m.memory_id = $5 OR (m.status = 'active' AND (
                    ({personal} AND m.kind = 'preference')
                    OR ({project} AND m.kind IN ('rule','decision','gotcha')
                        AND (m.repo_branch IS NULL OR m.repo_branch = $3
                             OR coalesce(m.repo_branch = (SELECT default_branch FROM projects WHERE project_id = $2),
                                         m.repo_branch IN ('main','master')))))))
              {} LIMIT $6",
            essential_order()
        ))
        .bind(person_id)
        .bind(&context.project_id)
        .bind(&context.branch)
        .bind(&shown)
        .bind(handoff)
        .bind(limit as i64)
        .fetch_all(&mut *tx)
        .await?;
        let essentials = essentials
            .iter()
            .map(memory_from_row)
            .collect::<Result<Vec<_>>>()?;
        let brief = session_brief(
            &pins,
            &essentials,
            cairn_core::id::now_seconds(),
            limit,
            budget,
        );
        finish_push(
            tx,
            person_id,
            session_id,
            Lookup::Brief.as_str(),
            &brief.memory_ids,
        )
        .await?;
        Ok(brief)
    }

    /// One exact command warning per session, or strict phrase recall on a failed command.
    /// An empty failure tail means the host only supplied a failure status.
    pub async fn command_recall(
        &self,
        person_id: &str,
        session_id: &str,
        project_id: Option<&str>,
        command: &str,
        failure: Option<&str>,
    ) -> Result<String> {
        let command = entities::normalize_command(&redact_sensitive_content(command));
        if command.is_empty() || command.contains('\0') || command.chars().count() > 200 {
            return Ok(String::new());
        }
        let tail = failure.map(error_tail);
        let phrase = match tail.as_deref() {
            Some(tail) if !tail.is_empty() => {
                let Some(line) = first_error_line(tail).filter(|line| !line.contains('\0')) else {
                    return Ok(String::new());
                };
                Some(line)
            }
            _ => None,
        };

        let unmuted = unmuted("m", "$1");
        let mut tx = self.pool.begin().await?;
        let session = Some(session_id);
        let shown = already_shown(&mut tx, person_id, session).await?;
        let error_source = phrase.map(|line| {
            let line = line
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            format!("error:{}", sha256_hex(&[&line]))
        });
        if let Some(source) = &error_source {
            repeat_error(&mut tx, person_id, session_id, source).await?;
        }
        let limit = SESSION_PUSH_LIMIT.saturating_sub(shown.len()).min(2);
        if limit == 0 {
            tx.commit().await?;
            return Ok(String::new());
        }
        // The ledger lock covers both the once-per-command check and the card reservation.
        let source = if failure.is_some() {
            error_source.unwrap_or_else(|| "recall".to_owned())
        } else {
            let source = format!("command:{}", sha256_hex(&[&command]));
            let sent: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM session_shown
                  WHERE person_id = $1 AND session_id = $2 AND source = $3)",
            )
            .bind(person_id)
            .bind(session_id)
            .bind(&source)
            .fetch_one(&mut *tx)
            .await?;
            if sent {
                return Ok(String::new());
            }
            source
        };

        let terms = "phraseto_tsquery('english', $6)";
        let exact_command = "m.memory_id IN
            (SELECT memory_id FROM entities WHERE kind = 'command' AND name = $3)";
        // Three adjacent lexemes keep generic failure lines out of automatic recall.
        let error_match = phrase.map_or_else(String::new, |_| {
            format!("AND numnode({terms}) >= 5 AND {}", search_matches(terms))
        });
        let sql = format!(
            "SELECT m.memory_id, m.kind, m.title, m.summary, EXTRACT(EPOCH FROM m.created_at)::bigint
               FROM memories m LEFT JOIN usage u ON u.memory_id = m.memory_id
              WHERE {} AND m.status = 'active' AND {PUSH_TRUST} AND {unmuted}
                AND (m.scope <> 'project' OR m.project_id = $2)
                AND m.kind IN ('gotcha', 'procedure') AND m.memory_id <> ALL($4)
                AND {exact_command} {error_match}
              ORDER BY (m.kind = 'gotcha') DESC, coalesce(u.helpful, 0) DESC,
                       m.updated_at DESC, m.memory_id
              LIMIT $5",
            visible("m", "$1"),
        );
        let query = sqlx::query_as(&sql)
            .bind(person_id)
            .bind(project_id)
            .bind(&command)
            .bind(&shown)
            .bind(limit as i64);
        let query = if let Some(phrase) = phrase {
            query.bind(phrase)
        } else {
            query
        };
        let rows: Vec<(String, String, String, String, i64)> = query.fetch_all(&mut *tx).await?;

        let now = cairn_core::id::now_seconds();
        let notes: Vec<_> = rows
            .iter()
            .map(|(_, kind, title, summary, created_at)| {
                let why = if phrase.is_some() {
                    "same error seen before".to_owned()
                } else {
                    format!("{kind} for `{command}`")
                };
                (
                    title.as_str(),
                    summary.as_str(),
                    why,
                    card_age(*created_at, now),
                )
            })
            .collect();
        let notes: Vec<_> = notes
            .iter()
            .map(|(t, s, why, age)| (*t, *s, why.as_str(), age.as_str()))
            .collect();
        let label = if failure.is_some() {
            "failed command"
        } else {
            "command"
        };
        let (text, count) = topic_notes(label, &notes);
        let ids: Vec<_> = rows.into_iter().take(count).map(|(id, ..)| id).collect();
        finish_push(tx, person_id, session, &source, &ids).await?;
        Ok(text)
    }

    /// Up to two memories matching a file, its module or a symbol in the tool input.
    /// Render a compact topic card when more than two match.
    pub async fn topic(
        &self,
        person_id: &str,
        path: &str,
        context: &WorkContext,
        session_id: Option<&str>,
    ) -> Result<Topic> {
        let unmuted = unmuted("m", "$1");
        let mut tx = self.pool.begin().await?;
        let shown = already_shown(&mut tx, person_id, session_id).await?;
        let limit = SESSION_PUSH_LIMIT.saturating_sub(shown.len()).min(2);
        if limit == 0 {
            tx.commit().await?;
            return Ok(Topic::default());
        }
        let symbols: Vec<_> = context
            .symbols
            .iter()
            .take(entities::ENTITY_CAP)
            .filter(|s| s.len() <= 200 && entities::is_symbol(s))
            .map(String::as_str)
            .collect();
        let rows = sqlx::query(&format!(
            "SELECT m.memory_id, m.kind, m.title, m.summary, count(*) OVER () AS total,
                    EXTRACT(EPOCH FROM m.created_at)::bigint AS created_at,
                    CASE WHEN bool_or(e.kind = 'file') THEN 'file ' || $2
                         WHEN bool_or(e.kind = 'symbol') THEN 'symbol ' || min(e.name) FILTER (WHERE e.kind = 'symbol')
                         ELSE 'module ' || $4 END AS why
               FROM memories m
               JOIN entities e ON e.memory_id = m.memory_id
               LEFT JOIN usage u ON u.memory_id = m.memory_id
              WHERE {} AND m.status = 'active' AND {PUSH_TRUST} AND {unmuted} AND m.kind <> 'handoff'
                AND m.memory_id <> ALL($6)
                AND ((e.kind = 'file' AND e.name = $2) OR (e.kind = 'module' AND e.name = $4)
                     OR (e.kind = 'symbol' AND e.name = ANY($5)
                         AND (m.scope <> 'project' OR m.project_id = $3)))
              GROUP BY m.memory_id
              ORDER BY CASE WHEN m.kind IN ('rule', 'gotcha') THEN 0
                            WHEN m.kind = 'decision' THEN 1
                            WHEN m.kind IN ('fact', 'procedure') THEN 2 ELSE 3 END,
                       coalesce(max(u.helpful), 0) DESC, m.updated_at DESC,
                       bool_or(e.kind = 'file') DESC,
                       (m.project_id IS NOT NULL AND m.project_id = $3) DESC,
                       m.memory_id
              LIMIT $7",
            visible("m", "$1")
        ))
        .bind(person_id)
        .bind(path)
        .bind(&context.project_id)
        .bind(module_of(path))
        .bind(&symbols)
        .bind(&shown)
        .bind(limit as i64)
        .fetch_all(&mut *tx)
        .await?;

        let now = cairn_core::id::now_seconds();
        let mut notes = Vec::new();
        let mut kinds = Vec::new();
        for row in &rows {
            kinds.push(
                row.try_get::<String, _>("kind")?
                    .parse::<Kind>()
                    .map_err(StoreError::Validation)?,
            );
            notes.push(TopicNote {
                id: row.try_get("memory_id")?,
                title: row.try_get("title")?,
                summary: row.try_get("summary")?,
                why: row.try_get("why")?,
                age: card_age(row.try_get("created_at")?, now),
            });
        }
        let total = rows
            .first()
            .map(|row| row.try_get::<i64, _>("total"))
            .transpose()?
            .unwrap_or(0);
        let (card, count) = if total >= 3 {
            let first: Vec<_> = kinds
                .into_iter()
                .zip(&notes)
                .map(|(kind, note)| {
                    (
                        kind,
                        note.title.as_str(),
                        note.why.as_str(),
                        note.age.as_str(),
                    )
                })
                .collect();
            let (card, count) = topic_card(path, total as usize, &first);
            (Some(card), count)
        } else {
            let text: Vec<_> = notes
                .iter()
                .map(|note| {
                    (
                        note.title.as_str(),
                        note.summary.as_str(),
                        note.why.as_str(),
                        note.age.as_str(),
                    )
                })
                .collect();
            (None, topic_notes(path, &text).1)
        };
        notes.truncate(count);
        let memory_ids: Vec<_> = notes.iter().map(|note| note.id.clone()).collect();
        finish_push(
            tx,
            person_id,
            session_id,
            Lookup::Topic.as_str(),
            &memory_ids,
        )
        .await?;
        if card.is_some() {
            notes.clear();
        }
        Ok(Topic {
            notes,
            card,
            memory_ids,
        })
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

#[cfg(test)]
mod tests;
