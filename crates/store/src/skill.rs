use crate::memory::visible;
use crate::{Result, Store, StoreError};
use cairn_core::capture::redact_sensitive_content;
use cairn_core::id::new_id;
use cairn_core::retrieve::prompt::PROMPT_QUERY_MAX_BYTES;
use cairn_core::skill::{BODY_MAX, DESCRIPTION_MAX, MAX_LIMIT, NAME_MAX, NOTE_MAX, TITLE_MAX};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::{PgConnection, Row};

const SEARCH_VECTOR: &str = "setweight(to_tsvector('english', v.title), 'A') ||
    setweight(to_tsvector('english', v.description), 'B') ||
    setweight(to_tsvector('english', v.body), 'D')";
const VOTE_COUNTS: &str = "CROSS JOIN LATERAL (
    SELECT count(*) FILTER (WHERE vote = 'helpful') AS helpful,
           count(*) FILTER (WHERE vote = 'wrong') AS wrong
      FROM skill_votes sv WHERE sv.skill_id = v.skill_id AND sv.version = v.version
) votes";
const VERSION_COLUMNS: &str = "v.version, v.author_id, v.change_note,
    extract(epoch FROM v.created_at)::bigint AS created_at";
const ARCHIVE_COLUMNS: &str =
    "extract(epoch FROM s.archived_at)::bigint AS archived_at, s.archived_by";

#[derive(Deserialize)]
pub struct SkillDraft {
    pub name: String,
    pub title: String,
    pub description: String,
    pub body: String,
    pub change_note: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Serialize)]
pub struct SkillCard {
    pub id: String,
    pub name: String,
    pub title: String,
    pub description: String,
    pub version: i32,
    pub score: f32,
    pub votes: SkillVotes,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Serialize)]
pub struct SkillSearchResults {
    pub results: Vec<SkillCard>,
    pub next_offset: Option<u32>,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Serialize)]
pub struct SkillVotes {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub helpful: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub wrong: i64,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Serialize)]
pub struct SkillVersion {
    pub version: i32,
    pub author_id: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub created_at: i64,
    pub change_note: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Serialize)]
pub struct SkillDetail {
    #[serde(flatten)]
    #[cfg_attr(feature = "ts", ts(flatten))]
    pub card: SkillCard,
    pub body: String,
    pub author_id: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub created_at: i64,
    pub change_note: String,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub archived_at: Option<i64>,
    pub archived_by: Option<String>,
    pub my_vote: Option<String>,
    pub my_note: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillVote {
    Helpful,
    Wrong,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Serialize)]
pub struct SkillSuggestion {
    pub memory_id: String,
    pub title: String,
    pub description: String,
}

fn card(row: &PgRow) -> Result<SkillCard> {
    Ok(SkillCard {
        id: row.try_get("skill_id")?,
        name: row.try_get("name")?,
        title: row.try_get("title")?,
        description: row.try_get("description")?,
        version: row.try_get("version")?,
        score: row.try_get("score")?,
        votes: SkillVotes {
            helpful: row.try_get("helpful")?,
            wrong: row.try_get("wrong")?,
        },
    })
}

fn text(field: &str, raw: &str, max: usize, multiline: bool) -> Result<String> {
    let value = if multiline { raw } else { raw.trim() };
    if value.trim().is_empty() || value.len() > max {
        return Err(StoreError::Validation(format!(
            "{field} must be between 1 and {max} bytes"
        )));
    }
    if value
        .chars()
        .any(|c| c.is_control() && !(multiline && matches!(c, '\n' | '\r' | '\t')))
    {
        return Err(StoreError::Validation(format!(
            "{field} contains control characters"
        )));
    }
    let redacted = redact_sensitive_content(value);
    if redacted.len() > max {
        return Err(StoreError::Validation(format!(
            "{field} exceeds {max} bytes after redaction"
        )));
    }
    Ok(redacted)
}

impl SkillDraft {
    fn validated(self) -> Result<Self> {
        if self.name.len() > NAME_MAX
            || !self.name.split('-').all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            })
            || redact_sensitive_content(&self.name) != self.name
        {
            return Err(StoreError::Validation(format!(
                "name must be 1 to {NAME_MAX} bytes of lowercase letters or digits separated by single hyphens"
            )));
        }
        Ok(Self {
            name: self.name,
            title: text("title", &self.title, TITLE_MAX, false)?,
            description: text("description", &self.description, DESCRIPTION_MAX, false)?,
            body: text("body", &self.body, BODY_MAX, true)?,
            change_note: text("change note", &self.change_note, NOTE_MAX, false)?,
        })
    }
}

async fn insert_version(
    db: &mut PgConnection,
    id: &str,
    version: i32,
    person: &str,
    draft: SkillDraft,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO skill_versions (skill_id, version, title, description, body, change_note, author_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(version)
    .bind(draft.title)
    .bind(draft.description)
    .bind(draft.body)
    .bind(draft.change_note)
    .bind(person)
    .execute(db)
    .await?;
    Ok(())
}

async fn lock_skill(db: &mut PgConnection, key: &str) -> Result<(String, String, i32, bool)> {
    sqlx::query_as(
        "SELECT skill_id, name, current_version, archived_at IS NOT NULL FROM skills
          WHERE skill_id = $1 OR name = $1 FOR UPDATE",
    )
    .bind(key)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| StoreError::NotFound("skill".into()))
}

fn active(archived: bool) -> Result<()> {
    if archived {
        return Err(StoreError::Conflict("skill is archived".into()));
    }
    Ok(())
}

impl Store {
    pub async fn search_skills(
        &self,
        query: &str,
        limit: u32,
        offset: u32,
        archived: bool,
    ) -> Result<SkillSearchResults> {
        if query.len() > PROMPT_QUERY_MAX_BYTES || query.contains('\0') {
            return Err(StoreError::Validation("invalid skill search query".into()));
        }
        let query = redact_sensitive_content(query.trim());
        let limit = limit.clamp(1, MAX_LIMIT);
        let rows = sqlx::query(&format!(
            "WITH q AS (SELECT websearch_to_tsquery('english', $1) AS terms)
             SELECT v.skill_id, s.name, v.title, v.description, v.version, votes.*,
                    CASE WHEN s.name = lower($1) THEN 1::float4 ELSE ts_rank_cd(({SEARCH_VECTOR}), q.terms, 32) END AS score
               FROM skills s JOIN skill_versions v ON (v.skill_id, v.version) = (s.skill_id, s.current_version)
               CROSS JOIN q {VOTE_COUNTS}
              WHERE (s.archived_at IS NOT NULL) = $4
                AND ($1 = '' OR s.name = lower($1) OR ({SEARCH_VECTOR}) @@ q.terms)
              ORDER BY score DESC, s.name LIMIT $2 OFFSET $3"
        ))
        .bind(query)
        .bind(i64::from(limit) + 1)
        .bind(i64::from(offset))
        .bind(archived)
        .fetch_all(&self.pool)
        .await?;
        let next_offset = if rows.len() > limit as usize {
            offset.checked_add(limit)
        } else {
            None
        };
        Ok(SkillSearchResults {
            results: rows
                .iter()
                .take(limit as usize)
                .map(card)
                .collect::<Result<_>>()?,
            next_offset,
        })
    }

    pub async fn get_skill(
        &self,
        person: &str,
        key: &str,
        version: Option<i32>,
        allow_archived: bool,
    ) -> Result<SkillDetail> {
        let row = sqlx::query(&format!(
            "SELECT v.skill_id, s.name, v.title, v.description, v.body, votes.*, 0::float4 AS score,
                    {VERSION_COLUMNS}, {ARCHIVE_COLUMNS}, mine.vote, mine.note
               FROM skills s JOIN skill_versions v ON v.skill_id = s.skill_id
               {VOTE_COUNTS}
               LEFT JOIN skill_votes mine ON (mine.skill_id, mine.version, mine.person_id) = (v.skill_id, v.version, $3)
              WHERE (s.skill_id = $1 OR s.name = $1) AND v.version = coalesce($2, s.current_version)"
        ))
        .bind(key)
        .bind(version)
        .bind(person)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound("skill version".into()))?;
        let archived_at: Option<i64> = row.try_get("archived_at")?;
        active(archived_at.is_some() && !allow_archived)?;
        Ok(SkillDetail {
            card: card(&row)?,
            body: row.try_get("body")?,
            author_id: row.try_get("author_id")?,
            created_at: row.try_get("created_at")?,
            change_note: row.try_get("change_note")?,
            archived_at,
            archived_by: row.try_get("archived_by")?,
            my_vote: row.try_get("vote")?,
            my_note: row.try_get("note")?,
        })
    }

    pub async fn skill_versions(&self, key: &str) -> Result<Vec<SkillVersion>> {
        let rows = sqlx::query(&format!(
            "SELECT {VERSION_COLUMNS} FROM skills s JOIN skill_versions v USING (skill_id)
              WHERE s.skill_id = $1 OR s.name = $1 ORDER BY v.version DESC"
        ))
        .bind(key)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(SkillVersion {
                    version: row.try_get("version")?,
                    author_id: row.try_get("author_id")?,
                    created_at: row.try_get("created_at")?,
                    change_note: row.try_get("change_note")?,
                })
            })
            .collect()
    }

    pub async fn save_skill(
        &self,
        person: &str,
        draft: SkillDraft,
        create_only: bool,
    ) -> Result<SkillDetail> {
        let draft = draft.validated()?;
        let mut tx = self.pool.begin().await?;
        let created: Option<String> = sqlx::query_scalar(
            "INSERT INTO skills (skill_id, name) VALUES ($1, $2)
             ON CONFLICT (name) DO NOTHING RETURNING skill_id",
        )
        .bind(new_id("skill"))
        .bind(&draft.name)
        .fetch_optional(&mut *tx)
        .await?;
        let (id, version) = match created {
            Some(id) => (id, 1),
            None if create_only => {
                return Err(StoreError::Conflict("skill name is already in use".into()));
            }
            None => {
                let (id, _, version, archived) = lock_skill(&mut tx, &draft.name).await?;
                active(archived)?;
                (id, version + 1)
            }
        };
        insert_version(&mut tx, &id, version, person, draft).await?;
        set_current(&mut tx, &id, version).await?;
        tx.commit().await?;
        self.get_skill(person, &id, Some(version), true).await
    }

    pub async fn roll_back_skill(
        &self,
        person: &str,
        key: &str,
        version: i32,
        change_note: &str,
    ) -> Result<SkillDetail> {
        let change_note = text("change note", change_note, NOTE_MAX, false)?;
        let mut tx = self.pool.begin().await?;
        let (id, _, latest, archived) = lock_skill(&mut tx, key).await?;
        active(archived)?;
        let next = latest + 1;
        let inserted = sqlx::query(
            "INSERT INTO skill_versions (skill_id, version, title, description, body, change_note, author_id)
             SELECT skill_id, $2, title, description, body, $3, $4 FROM skill_versions
              WHERE skill_id = $1 AND version = $5",
        )
        .bind(&id).bind(next).bind(change_note).bind(person).bind(version)
        .execute(&mut *tx).await?;
        if inserted.rows_affected() == 0 {
            return Err(StoreError::NotFound("skill version".into()));
        }
        set_current(&mut tx, &id, next).await?;
        tx.commit().await?;
        self.get_skill(person, &id, Some(next), true).await
    }

    pub async fn vote_for_skill(
        &self,
        person: &str,
        key: &str,
        version: i32,
        vote: SkillVote,
        note: Option<&str>,
    ) -> Result<SkillDetail> {
        let note = note
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| text("vote note", s, NOTE_MAX, false))
            .transpose()?;
        let vote = match vote {
            SkillVote::Helpful => "helpful",
            SkillVote::Wrong => "wrong",
        };
        let mut tx = self.pool.begin().await?;
        let (id, _, _, archived) = lock_skill(&mut tx, key).await?;
        active(archived)?;
        let saved = sqlx::query(
            "INSERT INTO skill_votes (skill_id, version, person_id, vote, note)
             SELECT skill_id, version, $3, $4, $5 FROM skill_versions WHERE skill_id = $1 AND version = $2
             ON CONFLICT (skill_id, version, person_id) DO UPDATE SET vote = EXCLUDED.vote, note = EXCLUDED.note",
        )
        .bind(&id).bind(version).bind(person).bind(vote).bind(note)
        .execute(&mut *tx).await?;
        if saved.rows_affected() == 0 {
            return Err(StoreError::NotFound("skill version".into()));
        }
        tx.commit().await?;
        self.get_skill(person, &id, Some(version), true).await
    }

    pub async fn set_skill_archive(&self, person: &str, key: &str, archive: bool) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let (id, name, _, archived) = lock_skill(&mut tx, key).await?;
        if archive != archived {
            sqlx::query(
                "UPDATE skills SET archived_at = CASE WHEN $3 THEN now() END,
                                   archived_by = CASE WHEN $3 THEN $2 END
                  WHERE skill_id = $1",
            )
            .bind(&id)
            .bind(person)
            .bind(archive)
            .execute(&mut *tx)
            .await?;
            crate::account::audit_tx(
                &mut tx,
                person,
                None,
                if archive {
                    "skill_archived"
                } else {
                    "skill_restored"
                },
                Some(&id),
                serde_json::json!({"name": name}),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

async fn set_current(db: &mut PgConnection, id: &str, version: i32) -> Result<()> {
    sqlx::query("UPDATE skills SET current_version = $2 WHERE skill_id = $1")
        .bind(id)
        .bind(version)
        .execute(db)
        .await?;
    Ok(())
}

const SUGGEST_FETCHED: i32 = 5;
const SUGGEST_HELPFUL: i32 = 2;

fn suggested() -> String {
    format!(
        "m.kind = 'procedure' AND m.status = 'active'
          AND (u.fetched >= {SUGGEST_FETCHED} OR u.helpful >= {SUGGEST_HELPFUL})
          AND jsonb_path_exists(m.fields, '$.steps[*] ? (@ like_regex \"[^[:space:]]\")')
          AND NOT EXISTS (SELECT 1 FROM skill_suggestion_dismissals d
                           WHERE d.memory_id = m.memory_id AND d.person_id = $1)
          AND {}",
        visible("m", "$1")
    )
}

impl Store {
    pub async fn skill_suggestions(&self, person: &str) -> Result<Vec<SkillSuggestion>> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(&format!(
            "SELECT m.memory_id, m.title, m.summary FROM memories m JOIN usage u USING (memory_id)
              WHERE {} AND NOT EXISTS (SELECT 1 FROM skills s WHERE s.source_memory_id = m.memory_id)
              ORDER BY u.helpful DESC, u.fetched DESC, m.memory_id LIMIT {MAX_LIMIT}",
            suggested()
        ))
        .bind(person)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(memory_id, title, description)| SkillSuggestion {
                memory_id,
                title,
                description,
            })
            .collect())
    }

    pub async fn dismiss_skill_suggestion(&self, person: &str, memory_id: &str) -> Result<()> {
        sqlx::query(&format!(
            "INSERT INTO skill_suggestion_dismissals (person_id, memory_id)
             SELECT $1, m.memory_id FROM memories m JOIN usage u USING (memory_id)
              WHERE m.memory_id = $2 AND {} ON CONFLICT DO NOTHING",
            suggested()
        ))
        .bind(person)
        .bind(memory_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn convert_skill_suggestion(
        &self,
        person: &str,
        memory_id: &str,
    ) -> Result<SkillDetail> {
        let mut tx = self.pool.begin().await?;
        let (title, description, steps): (String, String, serde_json::Value) = sqlx::query_as(&format!(
            "SELECT m.title, m.summary, m.fields -> 'steps' FROM memories m JOIN usage u USING (memory_id)
              WHERE m.memory_id = $2 AND {} FOR UPDATE OF m",
            suggested()
        ))
        .bind(person)
        .bind(memory_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StoreError::NotFound("skill suggestion".into()))?;
        if let Some(id) = sqlx::query_scalar::<_, String>(
            "SELECT skill_id FROM skills WHERE source_memory_id = $1",
        )
        .bind(memory_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            tx.commit().await?;
            return self.get_skill(person, &id, None, false).await;
        }
        let id = new_id("skill");
        let suffix = id.trim_start_matches("skill_");
        let slug = title
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>()
            .join("-")
            .to_ascii_lowercase();
        let slug = slug[..slug.len().min(NAME_MAX)].trim_end_matches('-');
        let name = if slug.is_empty() { "skill" } else { slug };
        let body = steps
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .filter(|step| !step.trim().is_empty())
            .enumerate()
            .map(|(i, step)| format!("{}. {}", i + 1, step.trim()))
            .collect::<Vec<_>>()
            .join("\n");
        let mut draft = SkillDraft {
            name: name.to_owned(),
            title: title.split_whitespace().collect::<Vec<_>>().join(" "),
            description: description.split_whitespace().collect::<Vec<_>>().join(" "),
            body,
            change_note: "Created from a procedure memory".into(),
        };
        draft
            .title
            .truncate(draft.title.floor_char_boundary(TITLE_MAX));
        draft
            .description
            .truncate(draft.description.floor_char_boundary(DESCRIPTION_MAX));
        let mut draft = draft.validated()?;
        let inserted = sqlx::query(
            "INSERT INTO skills (skill_id, name, source_memory_id) VALUES ($1, $2, $3)
             ON CONFLICT (name) DO NOTHING",
        )
        .bind(&id)
        .bind(&draft.name)
        .bind(memory_id)
        .execute(&mut *tx)
        .await?;
        if inserted.rows_affected() == 0 {
            let prefix = name[..name.len().min(NAME_MAX - 1 - suffix.len())].trim_end_matches('-');
            draft.name = format!("{prefix}-{suffix}");
            sqlx::query(
                "INSERT INTO skills (skill_id, name, source_memory_id) VALUES ($1, $2, $3)",
            )
            .bind(&id)
            .bind(&draft.name)
            .bind(memory_id)
            .execute(&mut *tx)
            .await?;
        }
        insert_version(&mut tx, &id, 1, person, draft).await?;
        tx.commit().await?;
        self.get_skill(person, &id, Some(1), true).await
    }
}

/// Coverage uses the same unembedded lexical score as memory prompt recall.
pub(crate) async fn prompt_skill(
    db: &mut PgConnection,
    prompt: &str,
    min_score: f32,
) -> Result<Option<(String, String, String)>> {
    Ok(sqlx::query_as(
        "WITH q AS (
           SELECT tsvector_to_array(to_tsvector('english', $1)) AS lexemes,
                  replace(plainto_tsquery('english', $1)::text, ' & ', ' | ')::tsquery AS any_term
         )
         SELECT s.skill_id, v.title, v.description FROM skills s
           JOIN skill_versions v ON (v.skill_id, v.version) = (s.skill_id, s.current_version)
           CROSS JOIN q
           CROSS JOIN LATERAL (
             SELECT count(*)::float4 / nullif(cardinality(q.lexemes), 0)::float4 AS score
               FROM unnest(v.card_vector) t WHERE t.lexeme = ANY(q.lexemes)
           ) matched
          WHERE s.archived_at IS NULL AND v.card_vector @@ q.any_term AND matched.score >= $2
          ORDER BY matched.score DESC, s.name LIMIT 1 FOR SHARE OF s",
    )
    .bind(prompt)
    .bind(min_score)
    .fetch_optional(db)
    .await?)
}
