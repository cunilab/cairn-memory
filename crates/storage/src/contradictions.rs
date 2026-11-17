use super::*;
use cairn_core::{
    EMBEDDING_MODEL, EMBEDDING_VERSION, GraphFilter, JobEnd, JobKind, LinkRelation, Memory,
    MemoryGraph, MemoryLink, MemoryScope, ReviewStatus, SIMILARITY_CANDIDATES,
    SIMILARITY_INTERVAL_SECONDS, SIMILARITY_JOB_KEY, SimilarityBand, SimilarityReview,
    build_memory_graph, embedding_fingerprint, graph_node_cap, ordered_pair, same_scope,
    similarity_candidates, supersede_allowed,
};
use sqlx::{Postgres, Transaction};
use std::collections::{HashMap, HashSet};

fn stamp_text(now: u64) -> String {
    now.to_string()
}

fn band_name(band: SimilarityBand) -> &'static str {
    match band {
        SimilarityBand::PossibleConflict => "possible_conflict",
        SimilarityBand::NearDuplicate => "near_duplicate",
    }
}

fn parse_band(value: &str) -> Result<SimilarityBand, PostgresError> {
    match value {
        "possible_conflict" => Ok(SimilarityBand::PossibleConflict),
        "near_duplicate" => Ok(SimilarityBand::NearDuplicate),
        other => Err(PostgresError::Validation(format!(
            "unknown similarity band: {other}"
        ))),
    }
}

fn status_name(status: ReviewStatus) -> &'static str {
    match status {
        ReviewStatus::Open => "open",
        ReviewStatus::Dismissed => "dismissed",
        ReviewStatus::Contradicted => "contradicted",
        ReviewStatus::Superseded => "superseded",
    }
}

fn parse_status(value: &str) -> Result<ReviewStatus, PostgresError> {
    match value {
        "open" => Ok(ReviewStatus::Open),
        "dismissed" => Ok(ReviewStatus::Dismissed),
        "contradicted" => Ok(ReviewStatus::Contradicted),
        "superseded" => Ok(ReviewStatus::Superseded),
        other => Err(PostgresError::Validation(format!(
            "unknown review status: {other}"
        ))),
    }
}

fn review_from_row(row: &PgRow) -> Result<SimilarityReview, PostgresError> {
    let created: i64 = row.try_get("created_at")?;
    let resolved: Option<i64> = row.try_get("resolved_at")?;
    Ok(SimilarityReview {
        review_id: row.try_get("review_id")?,
        left_id: row.try_get("left_id")?,
        right_id: row.try_get("right_id")?,
        scope: parse_scope(row.try_get("scope")?)?,
        project_id: row.try_get("project_id")?,
        cosine: row.try_get("cosine")?,
        band: parse_band(&row.try_get::<String, _>("band")?)?,
        status: parse_status(&row.try_get::<String, _>("status")?)?,
        created_at: u64::try_from(created).unwrap_or(0),
        resolved_at: resolved.and_then(|value| u64::try_from(value).ok()),
        resolved_by: row.try_get("resolved_by")?,
    })
}

fn may_edit(
    account: &cairn_core::Account,
    memory: &Memory,
    row_person: &str,
    project_owner: Option<&str>,
) -> Result<(), PostgresError> {
    match memory.scope {
        MemoryScope::Global => {
            if account.is_admin || row_person == account.person_id {
                Ok(())
            } else {
                Err(PostgresError::Validation(
                    "only the global memory author or administrator may edit".to_owned(),
                ))
            }
        }
        MemoryScope::Project => {
            let author = memory.published_by.as_deref().unwrap_or(row_person);
            if author == account.person_id || project_owner == Some(account.person_id.as_str()) {
                Ok(())
            } else {
                Err(PostgresError::Validation(
                    "only the decision author or project owner may edit".to_owned(),
                ))
            }
        }
        MemoryScope::Personal => {
            if row_person == account.person_id {
                Ok(())
            } else {
                Err(PostgresError::Validation(format!(
                    "unknown memory: {}",
                    memory.memory_id
                )))
            }
        }
    }
}

async fn project_owner(
    tx: &mut Transaction<'_, Postgres>,
    project_id: &str,
) -> Result<Option<String>, PostgresError> {
    sqlx::query_scalar("SELECT person_id FROM cairn_projects WHERE project_id = $1 FOR SHARE")
        .bind(project_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(PostgresError::from)
}

async fn lock_readable(
    tx: &mut Transaction<'_, Postgres>,
    person_id: &str,
    memory_id: &str,
) -> Result<(Memory, String), PostgresError> {
    let sql = format!(
        "{} WHERE m.memory_id = $2 AND {} AND {} AND {} FOR UPDATE OF m",
        memory_select_sql(),
        recall_access_sql(),
        live_memory_sql(),
        not_superseded_by_live_memory_sql(false)
    );
    let row = sqlx::query(&sql)
        .bind(person_id)
        .bind(memory_id)
        .fetch_optional(&mut **tx)
        .await?;
    let Some(row) = row else {
        return Err(PostgresError::Validation(format!(
            "unknown memory: {memory_id}"
        )));
    };
    Ok((memory_from_row(&row)?, row.try_get("person_id")?))
}

async fn close_open_review(
    tx: &mut Transaction<'_, Postgres>,
    left_id: &str,
    right_id: &str,
    status: ReviewStatus,
    person_id: &str,
    now: u64,
) -> Result<(), PostgresError> {
    let (left_id, right_id) = ordered_pair(left_id, right_id);
    sqlx::query(
        "UPDATE cairn_similarity_reviews
         SET status = $1, resolved_at = $2, resolved_by = $3
         WHERE status = 'open' AND left_id = $4 AND right_id = $5",
    )
    .bind(status_name(status))
    .bind(i64::try_from(now).unwrap_or(i64::MAX))
    .bind(person_id)
    .bind(left_id)
    .bind(right_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn parse_vector(text: &str) -> Option<Vec<f32>> {
    let text = text.trim().trim_start_matches('[').trim_end_matches(']');
    if text.is_empty() {
        return None;
    }
    let values = text
        .split(',')
        .map(|part| part.trim().parse::<f32>().ok())
        .collect::<Option<Vec<_>>>()?;
    (values.len() == 384 && values.iter().all(|value| value.is_finite())).then_some(values)
}

impl PostgresStore {
    async fn edit_pair(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        account: &cairn_core::Account,
        first_id: &str,
        second_id: &str,
        edit_both: bool,
        contradicted_id: Option<&str>,
    ) -> Result<((Memory, String), (Memory, String)), PostgresError> {
        let (earlier, later) = if first_id <= second_id {
            (first_id, second_id)
        } else {
            (second_id, first_id)
        };
        let earlier_row = lock_readable(tx, &account.person_id, earlier).await?;
        let later_row = lock_readable(tx, &account.person_id, later).await?;
        let (left, right) = if earlier_row.0.memory_id == first_id {
            (earlier_row, later_row)
        } else {
            (later_row, earlier_row)
        };
        same_scope(&left.0, &right.0)?;
        let owner = match left.0.project_id.as_deref() {
            Some(project_id) => project_owner(tx, project_id).await?,
            None => None,
        };
        if edit_both {
            may_edit(account, &left.0, &left.1, owner.as_deref())?;
            may_edit(account, &right.0, &right.1, owner.as_deref())?;
        } else if let Some(target) = contradicted_id {
            let (memory, person) = if left.0.memory_id == target {
                (&left.0, left.1.as_str())
            } else {
                (&right.0, right.1.as_str())
            };
            may_edit(account, memory, person, owner.as_deref())?;
        }
        Ok((left, right))
    }

    pub async fn declare_contradiction(
        &self,
        token: &str,
        contradicting_id: &str,
        contradicted_id: &str,
    ) -> Result<Memory, PostgresError> {
        if contradicting_id == contradicted_id {
            return Err(PostgresError::Validation(
                "a memory cannot contradict itself".to_owned(),
            ));
        }
        let account = self.authenticate_account(token).await?;
        let mut tx = self.pool.begin().await?;
        let ((mut source, _), _) = self
            .edit_pair(
                &mut tx,
                &account,
                contradicting_id,
                contradicted_id,
                false,
                Some(contradicted_id),
            )
            .await?;
        if !source.contradicts.iter().any(|id| id == contradicted_id) {
            source.contradicts.push(contradicted_id.to_owned());
            source.changed_by = Some(account.person_id.clone());
            source.changed_at = Some(stamp_text(now_seconds()));
        }
        sqlx::query(
            "UPDATE cairn_memories
             SET contradicts = $1, changed_by = $2, changed_at = $3
             WHERE memory_id = $4",
        )
        .bind(Json(&source.contradicts))
        .bind(&source.changed_by)
        .bind(&source.changed_at)
        .bind(contradicting_id)
        .execute(&mut *tx)
        .await?;
        close_open_review(
            &mut tx,
            contradicting_id,
            contradicted_id,
            ReviewStatus::Contradicted,
            &account.person_id,
            now_seconds(),
        )
        .await?;
        tx.commit().await?;
        Ok(source)
    }

    pub async fn supersede_memory(
        &self,
        token: &str,
        successor_id: &str,
        target_id: &str,
    ) -> Result<Memory, PostgresError> {
        if successor_id == target_id {
            return Err(PostgresError::Validation(
                "a memory cannot supersede itself".to_owned(),
            ));
        }
        let account = self.authenticate_account(token).await?;
        let mut tx = self.pool.begin().await?;
        let ((first, _), (second, _)) = self
            .edit_pair(&mut tx, &account, successor_id, target_id, true, None)
            .await?;
        let (mut successor, target) = if first.memory_id == successor_id {
            (first, second)
        } else {
            (second, first)
        };
        supersede_allowed(&successor, &target)?;
        successor.supersedes = Some(target_id.to_owned());
        successor.changed_by = Some(account.person_id.clone());
        successor.changed_at = Some(stamp_text(now_seconds()));
        sqlx::query(
            "UPDATE cairn_memories
             SET supersedes = $1, changed_by = $2, changed_at = $3
             WHERE memory_id = $4",
        )
        .bind(&successor.supersedes)
        .bind(&successor.changed_by)
        .bind(&successor.changed_at)
        .bind(successor_id)
        .execute(&mut *tx)
        .await?;
        close_open_review(
            &mut tx,
            successor_id,
            target_id,
            ReviewStatus::Superseded,
            &account.person_id,
            now_seconds(),
        )
        .await?;
        tx.commit().await?;
        Ok(successor)
    }

    pub async fn similarity_reviews(
        &self,
        token: &str,
    ) -> Result<Vec<SimilarityReview>, PostgresError> {
        let visible = self.memories(token).await?;
        let ids = visible
            .iter()
            .map(|memory| memory.memory_id.as_str())
            .collect::<HashSet<_>>();
        let rows = sqlx::query(
            "SELECT review_id, left_id, right_id, scope, project_id, cosine, band, status,
                    created_at, resolved_at, resolved_by
             FROM cairn_similarity_reviews
             WHERE status = 'open'
             ORDER BY review_id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(review_from_row)
            .filter(|review| {
                review.as_ref().is_ok_and(|review| {
                    ids.contains(review.left_id.as_str()) && ids.contains(review.right_id.as_str())
                }) || review.is_err()
            })
            .collect()
    }

    async fn open_review(
        &self,
        token: &str,
        review_id: &str,
    ) -> Result<SimilarityReview, PostgresError> {
        self.similarity_reviews(token)
            .await?
            .into_iter()
            .find(|review| review.review_id == review_id)
            .ok_or_else(|| PostgresError::Validation("unknown review".to_owned()))
    }

    pub async fn dismiss_review(&self, token: &str, review_id: &str) -> Result<(), PostgresError> {
        let review = self.open_review(token, review_id).await?;
        let account = self.authenticate_account(token).await?;
        let mut tx = self.pool.begin().await?;
        self.edit_pair(
            &mut tx,
            &account,
            &review.left_id,
            &review.right_id,
            true,
            None,
        )
        .await?;
        sqlx::query(
            "UPDATE cairn_similarity_reviews
             SET status = 'dismissed', resolved_at = $2, resolved_by = $3
             WHERE review_id = $1 AND status = 'open'",
        )
        .bind(review_id)
        .bind(i64::try_from(now_seconds()).unwrap_or(i64::MAX))
        .bind(&account.person_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn contradict_review(
        &self,
        token: &str,
        review_id: &str,
        contradicted_id: &str,
    ) -> Result<Memory, PostgresError> {
        let review = self.open_review(token, review_id).await?;
        let contradicting_id = if review.left_id == contradicted_id {
            review.right_id
        } else if review.right_id == contradicted_id {
            review.left_id
        } else {
            return Err(PostgresError::Validation(
                "contradicted memory is not in this review".to_owned(),
            ));
        };
        self.declare_contradiction(token, &contradicting_id, contradicted_id)
            .await
    }

    pub async fn supersede_review(
        &self,
        token: &str,
        review_id: &str,
        successor_id: &str,
    ) -> Result<Memory, PostgresError> {
        let review = self.open_review(token, review_id).await?;
        let target_id = if review.left_id == successor_id {
            review.right_id
        } else if review.right_id == successor_id {
            review.left_id
        } else {
            return Err(PostgresError::Validation(
                "successor memory is not in this review".to_owned(),
            ));
        };
        self.supersede_memory(token, successor_id, &target_id).await
    }

    pub async fn ensure_similarity_job(&self, now: u64) -> Result<(), PostgresError> {
        self.enqueue_job(JobKind::Similarity, SIMILARITY_JOB_KEY, now, false)
            .await?;
        Ok(())
    }

    async fn similarity_found(&self, now: u64) -> Result<Vec<SimilarityReview>, PostgresError> {
        let sql = format!(
            "{}, e.model_id AS embed_model, e.model_version AS embed_version, e.fingerprint AS embed_fingerprint
             {} LEFT JOIN cairn_memory_embeddings e ON e.memory_id = m.memory_id
             WHERE m.kind <> 'capture' AND {} AND {}",
            memory_select_sql().trim_end_matches("FROM cairn_memories m"),
            "FROM cairn_memories m",
            live_memory_sql(),
            not_superseded_by_live_memory_sql(false)
        );
        let rows = sqlx::query(&sql).fetch_all(&self.pool).await?;
        let mut ranked = Vec::new();
        for row in &rows {
            let memory = memory_from_row(row)?;
            let model: Option<String> = row.try_get("embed_model")?;
            let version: Option<String> = row.try_get("embed_version")?;
            let fingerprint: Option<String> = row.try_get("embed_fingerprint")?;
            if model.as_deref() == Some(EMBEDDING_MODEL)
                && version.as_deref() == Some(EMBEDDING_VERSION)
                && fingerprint.as_deref() == Some(embedding_fingerprint(&memory).as_str())
            {
                ranked.push(memory);
            }
        }
        ranked.sort_by(|left, right| {
            left.access_count
                .cmp(&right.access_count)
                .then(left.created_at.cmp(&right.created_at))
                .then(left.memory_id.cmp(&right.memory_id))
        });
        ranked.truncate(SIMILARITY_CANDIDATES);
        let ids = ranked
            .iter()
            .map(|memory| memory.memory_id.clone())
            .collect::<Vec<_>>();
        let vectors = if ids.is_empty() {
            HashMap::new()
        } else {
            let rows = sqlx::query(
                "SELECT memory_id, embedding::text AS embedding FROM cairn_memory_embeddings WHERE memory_id = ANY($1)",
            )
            .bind(&ids)
            .fetch_all(&self.pool)
            .await?;
            let mut vectors = HashMap::new();
            for row in &rows {
                let id: String = row.try_get("memory_id")?;
                let text: String = row.try_get("embedding")?;
                if let Some(vector) = parse_vector(&text) {
                    vectors.insert(id, vector);
                }
            }
            vectors
        };
        let stored = sqlx::query("SELECT left_id, right_id FROM cairn_similarity_reviews")
            .fetch_all(&self.pool)
            .await?;
        let existing = stored
            .iter()
            .map(|row| {
                Ok((
                    row.try_get::<String, _>("left_id")?,
                    row.try_get::<String, _>("right_id")?,
                ))
            })
            .collect::<Result<HashSet<_>, PostgresError>>()?;
        Ok(similarity_candidates(&ranked, &vectors, &existing, now))
    }

    pub async fn finish_similarity_scan(
        &self,
        claim: &cairn_core::BackgroundJob,
        now: u64,
    ) -> Result<usize, PostgresError> {
        let found = self.similarity_found(now).await?;
        let claim_id = claim.claim_id.clone().unwrap_or_default();
        let mut tx = self.pool.begin().await?;
        if !super::jobs::claim_is_held(&mut tx, &claim.job_id, &claim_id).await? {
            tx.rollback().await?;
            return Ok(0);
        }
        let mut inserted_ids = Vec::new();
        for review in &found {
            let inserted = sqlx::query(
                "INSERT INTO cairn_similarity_reviews
                    (review_id, left_id, right_id, scope, project_id, cosine, band, status, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, 'open', $8)
                 ON CONFLICT (left_id, right_id) DO NOTHING",
            )
            .bind(&review.review_id)
            .bind(&review.left_id)
            .bind(&review.right_id)
            .bind(scope_name(&review.scope))
            .bind(&review.project_id)
            .bind(review.cosine)
            .bind(band_name(review.band))
            .bind(i64::try_from(review.created_at).unwrap_or(i64::MAX))
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if inserted == 1 {
                inserted_ids.push(review.review_id.clone());
            }
        }
        if !inserted_ids.is_empty() {
            sqlx::query(
                "DELETE FROM cairn_similarity_reviews AS review
                 WHERE review.review_id = ANY($1)
                   AND EXISTS (
                       SELECT 1 FROM cairn_memories AS memory
                       WHERE (memory.memory_id = review.left_id AND (
                                jsonb_exists(memory.contradicts, review.right_id)
                                OR memory.supersedes = review.right_id))
                          OR (memory.memory_id = review.right_id AND (
                                jsonb_exists(memory.contradicts, review.left_id)
                                OR memory.supersedes = review.left_id))
                   )",
            )
            .bind(&inserted_ids)
            .execute(&mut *tx)
            .await?;
        }
        let kept: i64 = if inserted_ids.is_empty() {
            0
        } else {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM cairn_similarity_reviews WHERE review_id = ANY($1)",
            )
            .bind(&inserted_ids)
            .fetch_one(&mut *tx)
            .await?
        };
        let settled =
            super::jobs::settle_on(&mut tx, &claim.job_id, &claim_id, now, JobEnd::Succeeded)
                .await?;
        if !settled {
            tx.rollback().await?;
            return Ok(0);
        }
        super::jobs::enqueue_on(
            &mut tx,
            JobKind::Similarity,
            SIMILARITY_JOB_KEY,
            now.saturating_add(SIMILARITY_INTERVAL_SECONDS),
            false,
        )
        .await?;
        tx.commit().await?;
        Ok(usize::try_from(kept).unwrap_or(0))
    }

    pub async fn link_memories(
        &self,
        token: &str,
        source_id: &str,
        relation: LinkRelation,
        target_id: &str,
    ) -> Result<Memory, PostgresError> {
        self.write_link(token, source_id, relation, target_id, true)
            .await
    }

    pub async fn unlink_memories(
        &self,
        token: &str,
        source_id: &str,
        relation: LinkRelation,
        target_id: &str,
    ) -> Result<Memory, PostgresError> {
        self.write_link(token, source_id, relation, target_id, false)
            .await
    }

    async fn write_link(
        &self,
        token: &str,
        source_id: &str,
        relation: LinkRelation,
        target_id: &str,
        add: bool,
    ) -> Result<Memory, PostgresError> {
        if source_id == target_id {
            return Err(PostgresError::Validation(
                "a memory cannot link to itself".to_owned(),
            ));
        }
        let account = self.authenticate_account(token).await?;
        let mut tx = self.pool.begin().await?;
        let (first_id, second_id) = if source_id <= target_id {
            (source_id, target_id)
        } else {
            (target_id, source_id)
        };
        let first = lock_readable(&mut tx, &account.person_id, first_id).await?;
        let second = lock_readable(&mut tx, &account.person_id, second_id).await?;
        let (mut source, source_person, target) = if first.0.memory_id == source_id {
            (first.0, first.1, second.0)
        } else {
            (second.0, second.1, first.0)
        };
        if source.scope != target.scope || source.project_id != target.project_id {
            return Err(PostgresError::Validation(
                "link must stay in the same scope and project".to_owned(),
            ));
        }
        let owner = match source.project_id.as_deref() {
            Some(project_id) => project_owner(&mut tx, project_id).await?,
            None => None,
        };
        may_edit(&account, &source, &source_person, owner.as_deref())?;
        let link = MemoryLink {
            relation,
            target_id: target_id.to_owned(),
        };
        let present = source.links.iter().any(|existing| existing == &link);
        if add && !present {
            source.links.push(link);
            source.changed_by = Some(account.person_id.clone());
            source.changed_at = Some(stamp_text(now_seconds()));
        } else if !add && present {
            source.links.retain(|existing| existing != &link);
            source.changed_by = Some(account.person_id.clone());
            source.changed_at = Some(stamp_text(now_seconds()));
        }
        sqlx::query(
            "UPDATE cairn_memories
             SET links = $1, changed_by = $2, changed_at = $3
             WHERE memory_id = $4",
        )
        .bind(Json(&source.links))
        .bind(&source.changed_by)
        .bind(&source.changed_at)
        .bind(source_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(source)
    }

    pub async fn memory_graph(
        &self,
        token: &str,
        filter: &GraphFilter,
        cap: Option<usize>,
    ) -> Result<MemoryGraph, PostgresError> {
        let cap = graph_node_cap(cap)?;
        let memories = self.memories(token).await?;
        Ok(build_memory_graph(&memories, filter, cap))
    }
}
