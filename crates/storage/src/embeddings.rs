use super::*;
use cairn_core::{
    EMBEDDING_DIMENSIONS, EMBEDDING_MODEL, EMBEDDING_VERSION, MemoryEmbedding,
    contradiction_counts, contradiction_factor, hybrid_rank, now_seconds, ranking_multiplier,
    validate_vector,
};

const SOURCE_REVISION: &str = "encode(sha256(convert_to(jsonb_build_array(m.content,m.record,m.context)::text,'UTF8')),'hex')";

impl PostgresStore {
    /// Internal bounded server job input; no memory content is sent to another service.
    pub async fn embedding_jobs(&self, limit: usize) -> Result<Vec<Memory>, PostgresError> {
        let sql = format!("{} LEFT JOIN cairn_memory_embeddings e ON e.memory_id=m.memory_id
            WHERE {} AND {} AND (e.model_id IS DISTINCT FROM $1 OR e.model_version IS DISTINCT FROM $2
                OR e.source_revision IS DISTINCT FROM {})
                AND NOT EXISTS (
                    SELECT 1 FROM cairn_job_quarantine q
                    WHERE q.kind = 'embed' AND q.job_key = m.memory_id
                )
            ORDER BY m.created_at,m.memory_id LIMIT $3",
            memory_select_sql(),live_memory_sql(),not_superseded_by_live_memory_sql(false),SOURCE_REVISION);
        let rows =
            sqlx::query(&sql)
                .bind(EMBEDDING_MODEL)
                .bind(EMBEDDING_VERSION)
                .bind(i64::try_from(limit.min(512)).map_err(|_| {
                    PostgresError::Validation("invalid embedding batch size".into())
                })?)
                .fetch_all(&self.pool)
                .await?;
        rows.iter().map(memory_from_row).collect()
    }

    pub async fn memories_needing_embedding(
        &self,
        ids: &[String],
    ) -> Result<Vec<Memory>, PostgresError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let sql = format!(
            "{} LEFT JOIN cairn_memory_embeddings e ON e.memory_id=m.memory_id
            WHERE {} AND {} AND m.memory_id = ANY($3) AND (e.model_id IS DISTINCT FROM $1 OR e.model_version IS DISTINCT FROM $2
                OR e.source_revision IS DISTINCT FROM {})",
            memory_select_sql(),
            live_memory_sql(),
            not_superseded_by_live_memory_sql(false),
            SOURCE_REVISION
        );
        let rows = sqlx::query(&sql)
            .bind(EMBEDDING_MODEL)
            .bind(EMBEDDING_VERSION)
            .bind(ids)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(memory_from_row).collect()
    }

    pub async fn put_embedding(&self, embedding: MemoryEmbedding) -> Result<bool, PostgresError> {
        validate_vector(&embedding.vector)?;
        let mut tx = self.pool.begin().await?;
        let sql = format!(
            "{} WHERE m.memory_id=$1 AND {} AND {} FOR UPDATE OF m",
            memory_select_sql(),
            live_memory_sql(),
            not_superseded_by_live_memory_sql(false)
        );
        let row = sqlx::query(&sql)
            .bind(&embedding.memory_id)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else { return Ok(false) };
        if !embedding.matches(&memory_from_row(&row)?) {
            return Ok(false);
        }
        let revision: String = sqlx::query_scalar(&format!(
            "SELECT {SOURCE_REVISION} FROM cairn_memories m WHERE m.memory_id=$1"
        ))
        .bind(&embedding.memory_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO cairn_memory_embeddings (memory_id,model_id,model_version,dimensions,normalized,fingerprint,source_revision,created_at,embedding)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,($9::text)::vector)
            ON CONFLICT (memory_id) DO UPDATE SET model_id=EXCLUDED.model_id,model_version=EXCLUDED.model_version,
            dimensions=EXCLUDED.dimensions,normalized=EXCLUDED.normalized,fingerprint=EXCLUDED.fingerprint,
            source_revision=EXCLUDED.source_revision,created_at=EXCLUDED.created_at,embedding=EXCLUDED.embedding")
            .bind(&embedding.memory_id).bind(&embedding.model_id).bind(&embedding.model_version).bind(EMBEDDING_DIMENSIONS as i32)
            .bind(embedding.normalized).bind(&embedding.fingerprint).bind(revision)
            .bind(embedding.created_at as i64).bind(serde_json::to_string(&embedding.vector)?).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }

    pub async fn search_with_vector(
        &self,
        token: &str,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
        vector: Option<&[f32]>,
    ) -> Result<Vec<RecallResult>, PostgresError> {
        let mut results = self.hybrid_matches(token, query, filter, vector).await?;
        results.truncate(limit.min(MAX_SEARCH_RESULTS as usize));
        self.record_recall(&results).await?;
        Ok(results)
    }

    pub async fn context_with_vector(
        &self,
        token: &str,
        query: &str,
        limit: usize,
        filter: &RecallFilter,
        vector: Option<&[f32]>,
        why: bool,
    ) -> Result<ContextPack, PostgresError> {
        let mut results = self.hybrid_matches(token, query, filter, vector).await?;
        if why {
            results.sort_by(|a, b| {
                b.decision_reason
                    .is_some()
                    .cmp(&a.decision_reason.is_some())
                    .then_with(|| b.relationships.len().cmp(&a.relationships.len()))
                    .then_with(|| b.score.total_cmp(&a.score))
            });
        }
        let total = results.len();
        results.truncate(MAX_SEARCH_RESULTS as usize);
        let pack = pack_window(
            if why {
                format!("why changed: {query}")
            } else {
                query.to_owned()
            },
            &mut results,
            limit,
            total,
        );
        self.record_recall(&pack.results).await?;
        Ok(pack)
    }

    pub(super) async fn hybrid_matches(
        &self,
        token: &str,
        query: &str,
        filter: &RecallFilter,
        vector: Option<&[f32]>,
    ) -> Result<Vec<RecallResult>, PostgresError> {
        let account = self.authenticate_account(token).await?;
        if query.trim().is_empty() {
            return Err(PostgresError::Validation("query must not be empty".into()));
        }
        let mut memories = self.all_memories_for_person(&account.person_id).await?;
        let counts = contradiction_counts(&memories);
        memories.retain(|m| memory_matches_filter(m, filter));
        if !filter.include_captures {
            memories.retain(|memory| memory.kind != cairn_core::MemoryKind::Capture);
        }
        let ids = memories
            .iter()
            .map(|m| m.memory_id.clone())
            .collect::<Vec<_>>();
        let rows=sqlx::query("SELECT m.memory_id,ts_rank_cd(m.search_tsv,plainto_tsquery('simple',$2)) AS score
            FROM cairn_memories m WHERE m.memory_id=ANY($1) AND m.search_tsv @@ plainto_tsquery('simple',$2)")
            .bind(&ids).bind(query).fetch_all(&self.pool).await?;
        let lexical = rows
            .iter()
            .map(|r| Ok((r.try_get("memory_id")?, r.try_get("score")?)))
            .collect::<Result<Vec<(String, f32)>, PostgresError>>()?;
        let semantic = if let Some(vector) = vector {
            validate_vector(vector)?;
            let sql=format!("SELECT m.memory_id,(1-(e.embedding <=> ($2::text)::vector))::real AS score
                FROM cairn_memory_embeddings e JOIN cairn_memories m USING(memory_id)
                WHERE m.memory_id=ANY($1) AND e.model_id=$3 AND e.model_version=$4 AND e.source_revision={SOURCE_REVISION}
                AND 1-(e.embedding <=> ($2::text)::vector)>0 ORDER BY e.embedding <=> ($2::text)::vector,m.memory_id LIMIT 512");
            sqlx::query(&sql)
                .bind(&ids)
                .bind(serde_json::to_string(vector)?)
                .bind(EMBEDDING_MODEL)
                .bind(EMBEDDING_VERSION)
                .fetch_all(&self.pool)
                .await?
                .iter()
                .map(|r| Ok((r.try_get("memory_id")?, r.try_get("score")?)))
                .collect::<Result<Vec<(String, f32)>, PostgresError>>()?
        } else {
            Vec::new()
        };
        let ranked = hybrid_rank(&memories, query, filter, Some(lexical), semantic)?;
        let by_id = memories
            .iter()
            .map(|m| (m.memory_id.as_str(), m))
            .collect::<HashMap<_, _>>();
        let now = now_seconds();
        let mut results = ranked
            .into_iter()
            .map(|(id, score)| {
                let memory = by_id[id.as_str()];
                let shaded = contradiction_factor(counts.get(id.as_str()).copied().unwrap_or(0));
                recall_result(
                    memory,
                    score * ranking_multiplier(memory, now) * shaded,
                    &memories,
                    &HashSet::new(),
                    filter,
                )
            })
            .collect::<Vec<_>>();
        cairn_core::prefer_decision(&mut results, &memories);
        prefer_newer_contradiction(&mut results, &memories);
        if filter.relevance_floor {
            results.retain(|hit| {
                by_id
                    .get(hit.memory_id.as_str())
                    .is_some_and(|memory| cairn_core::passes_relevance_floor(memory, query))
            });
        }
        Ok(results)
    }

    async fn record_recall(&self, results: &[RecallResult]) -> Result<(), PostgresError> {
        if results.is_empty() {
            return Ok(());
        }
        let ids = results
            .iter()
            .map(|result| result.memory_id.clone())
            .collect::<Vec<_>>();
        sqlx::query(
            "UPDATE cairn_memories
             SET access_count = access_count + 1,
                 last_accessed_at = to_timestamp($2::double precision)
             WHERE memory_id = ANY($1)",
        )
        .bind(&ids)
        .bind(now_seconds() as f64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
