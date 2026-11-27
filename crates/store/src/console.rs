//! Numbered console browse. Exact match is a literal substring; related reuses English full text.
//!
//! shortcut: exact match scans the visible rows; add a pg_trgm index if a console has more than
//! a few thousand memories and this query shows up in the slow log.

use crate::memory::{
    MEMORY_SELECT_WITH_DETAIL, active_project_memory, collection_memory, owned, search_matches,
    visible_with_archive,
};
use crate::view::PinQuota;
use crate::{Result, Store, StoreError};
use cairn_core::memory::{Kind, Memory, Scope, Status, TrustLevel};
use cairn_core::retrieve::brief::{PERSONAL_PIN_QUOTA, PROJECT_PIN_QUOTA};
use sqlx::Row;

const TRUST: &str = "CASE
    WHEN m.status = 'stale' OR u.wrong > 0 THEN 'doubted'
    WHEN m.rank = 'candidate' THEN 'new'
    WHEN u.helpful >= 2 OR m.proof_count >= 3 THEN 'proven'
    ELSE 'normal'
END";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleMatch {
    Browse,
    Exact,
    Related,
}

#[derive(Clone, Debug)]
pub struct ConsoleFilter {
    pub project_id: Option<String>,
    pub scope: Option<Scope>,
    pub kind: Option<Kind>,
    pub statuses: Vec<Status>,
    pub trust: Option<TrustLevel>,
    pub branch: Option<String>,
    /// Already trimmed. `None` is browse mode.
    pub q: Option<String>,
    pub mode: ConsoleMatch,
    pub pinned: bool,
    pub page: i64,
    pub page_size: i64,
    pub see_forgotten: bool,
    /// A console request. Archived project rows appear when a project is selected, or on Pinned
    /// so every occupied slot is visible.
    pub include_archived: bool,
}

pub struct ConsoleRows {
    pub memories: Vec<Memory>,
    pub page: i64,
    pub page_size: i64,
    pub total: i64,
    pub quotas: Vec<PinQuota>,
}

fn literal_predicate(param: &str) -> String {
    let hit = |expr: &str| {
        format!("strpos(lower(normalize({expr}, NFC)), lower(normalize({param}, NFC))) > 0")
    };
    format!(
        "({title} OR {summary} OR {reason} OR {detail} OR EXISTS (
            SELECT 1 FROM jsonb_path_query(m.fields, '$.** ? (@.type() == \"string\")') AS field_text(value)
            WHERE {field}
        ))",
        title = hit("m.title"),
        summary = hit("m.summary"),
        reason = hit("m.reason"),
        detail = hit("m.detail"),
        field = hit("field_text.value #>> '{{}}'"),
    )
}

fn scope_sql(filter: &ConsoleFilter) -> String {
    let include_archived =
        filter.include_archived && (filter.pinned || filter.project_id.is_some());
    if filter.see_forgotten {
        let owned = owned("m", "$1");
        if include_archived {
            owned
        } else {
            format!("{owned} AND {}", active_project_memory("m"))
        }
    } else {
        visible_with_archive("m", "$1", include_archived)
    }
}

fn where_sql(filter: &ConsoleFilter) -> String {
    let predicate = match filter.mode {
        ConsoleMatch::Related => search_matches("websearch_to_tsquery('english', $7)"),
        ConsoleMatch::Exact | ConsoleMatch::Browse => literal_predicate("$7"),
    };
    format!(
        "{scope} AND ($2::text IS NULL OR m.project_id = $2)
           AND ($3::text IS NULL OR m.scope = $3) AND ($4::text IS NULL OR m.kind = $4)
           AND (cardinality($5::text[]) = 0 OR m.status = ANY($5))
           AND ($6::text IS NULL OR m.repo_branch = $6)
           AND ($7::text IS NULL OR {predicate})
           AND ($8::text IS NULL OR ({TRUST}) = $8)
           AND (NOT $9 OR m.pinned_at IS NOT NULL)",
        scope = scope_sql(filter),
    )
}

fn order_sql(filter: &ConsoleFilter) -> &'static str {
    if filter.pinned {
        "(m.scope = 'personal') DESC, coalesce(proj.name, ''), m.pin_position, m.memory_id"
    } else if filter.q.is_some() && filter.mode == ConsoleMatch::Exact {
        "(lower(normalize(m.title, NFC)) = lower(normalize($7, NFC))) DESC, m.updated_at DESC, m.memory_id DESC"
    } else if filter.q.is_some() && filter.mode == ConsoleMatch::Related {
        "ts_rank_cd((m.search_tsv || to_tsvector('english', m.detail)), websearch_to_tsquery('english', $7), 32) DESC, m.updated_at DESC, m.memory_id DESC"
    } else {
        "m.updated_at DESC, m.memory_id DESC"
    }
}

fn bind<'a>(
    sql: &'a str,
    person_id: &'a str,
    filter: &'a ConsoleFilter,
    statuses: &'a [String],
) -> sqlx::query::Query<'a, sqlx::Postgres, sqlx::postgres::PgArguments> {
    sqlx::query(sql)
        .bind(person_id)
        .bind(&filter.project_id)
        .bind(filter.scope.map(Scope::as_str))
        .bind(filter.kind.map(Kind::as_str))
        .bind(statuses)
        .bind(&filter.branch)
        .bind(&filter.q)
        .bind(filter.trust.map(TrustLevel::as_str))
        .bind(filter.pinned)
}

impl Store {
    /// One page of memories the person can see, plus pin quotas when `pinned` is set.
    /// Count and rows share one repeatable-read snapshot.
    pub async fn console_memories(
        &self,
        person_id: &str,
        filter: &ConsoleFilter,
    ) -> Result<ConsoleRows> {
        if filter.page < 1 || !matches!(filter.page_size, 25 | 50 | 100) {
            return Err(StoreError::Validation(
                "page starts at 1 and page size is 25, 50 or 100".into(),
            ));
        }
        let statuses: Vec<String> = filter
            .statuses
            .iter()
            .map(|status| status.as_str().to_owned())
            .collect();
        let where_sql = where_sql(filter);
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await?;
        let total: i64 = bind(
            &format!(
                "SELECT count(*)::bigint AS total FROM memories m LEFT JOIN usage u ON u.memory_id = m.memory_id WHERE {where_sql}"
            ),
            person_id,
            filter,
            &statuses,
        )
        .fetch_one(&mut *tx)
        .await?
        .try_get("total")?;
        let pages = if total == 0 {
            1
        } else {
            (total + filter.page_size - 1) / filter.page_size
        };
        let page = filter.page.min(pages);
        let offset = (page - 1) * filter.page_size;
        let rows = bind(
            &format!(
                "{MEMORY_SELECT_WITH_DETAIL}
                 LEFT JOIN projects proj ON proj.project_id = m.project_id
                 WHERE {where_sql}
                 ORDER BY {order}
                 LIMIT $10 OFFSET $11",
                order = order_sql(filter),
            ),
            person_id,
            filter,
            &statuses,
        )
        .bind(filter.page_size)
        .bind(offset)
        .fetch_all(&mut *tx)
        .await?;
        // Quotas describe the whole bucket, including when the list filters hide rows.
        let quotas = quotas(&mut tx, person_id, filter.project_id.as_deref()).await?;
        tx.commit().await?;
        let mut memories = rows
            .iter()
            .filter_map(|row| collection_memory(row).transpose())
            .collect::<Result<Vec<_>>>()?;
        let include_archived =
            filter.include_archived && (filter.pinned || filter.project_id.is_some());
        self.attach(person_id, &mut memories, include_archived)
            .await?;
        Ok(ConsoleRows {
            memories,
            page,
            page_size: filter.page_size,
            total,
            quotas,
        })
    }
}

async fn quotas(
    tx: &mut sqlx::PgConnection,
    person_id: &str,
    project_id: Option<&str>,
) -> Result<Vec<PinQuota>> {
    if let Some(project_id) = project_id {
        let name: Option<String> = sqlx::query_scalar(
            "SELECT p.name FROM projects p
              JOIN project_members m USING (project_id)
             WHERE p.project_id = $1 AND m.person_id = $2",
        )
        .bind(project_id)
        .bind(person_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(name) = name else {
            return Ok(Vec::new());
        };
        let used: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM memories
              WHERE pinned_at IS NOT NULL AND scope = 'project' AND project_id = $1
                AND status <> 'forgotten'",
        )
        .bind(project_id)
        .fetch_one(&mut *tx)
        .await?;
        return Ok(vec![PinQuota {
            scope: "project".into(),
            project_id: Some(project_id.to_owned()),
            name,
            used,
            limit: PROJECT_PIN_QUOTA,
        }]);
    }
    let personal: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memories
          WHERE pinned_at IS NOT NULL AND scope = 'personal' AND owner_id = $1
            AND status <> 'forgotten'",
    )
    .bind(person_id)
    .fetch_one(&mut *tx)
    .await?;
    let mut quotas = vec![PinQuota {
        scope: "personal".into(),
        project_id: None,
        name: "Personal".into(),
        used: personal,
        limit: PERSONAL_PIN_QUOTA,
    }];
    let rows = sqlx::query(
        "SELECT m.project_id, p.name, count(*) AS used
           FROM memories m
           JOIN projects p ON p.project_id = m.project_id
           JOIN project_members pm ON pm.project_id = m.project_id AND pm.person_id = $1
          WHERE m.pinned_at IS NOT NULL AND m.scope = 'project' AND m.status <> 'forgotten'
          GROUP BY m.project_id, p.name
          ORDER BY p.name, m.project_id",
    )
    .bind(person_id)
    .fetch_all(&mut *tx)
    .await?;
    for row in rows {
        quotas.push(PinQuota {
            scope: "project".into(),
            project_id: row.try_get("project_id")?,
            name: row.try_get("name")?,
            used: row.try_get("used")?,
            limit: PROJECT_PIN_QUOTA,
        });
    }
    Ok(quotas)
}
