//! A memory's session link. Written only when a save or distill creates the row.

use crate::memory::{visible, visible_with_archive};
use crate::project::SessionReport;
use crate::{Result, Store, StoreError};
use serde::Serialize;
use sqlx::{PgConnection, Row};

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MemoryProvenance {
    pub person_id: String,
    pub session_id: String,
    pub project_id: Option<String>,
    pub branch: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub started_at: i64,
    /// `saved` or `distilled`.
    pub role: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionOutput {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub role: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionInspection {
    pub session: SessionReport,
    pub project_id: Option<String>,
    pub outputs: Vec<SessionOutput>,
    pub fixes: Vec<SessionOutput>,
}

/// Insert a link when the session row exists. A missing session or an existing memory is a no-op.
pub(crate) async fn record_output(
    tx: &mut PgConnection,
    person_id: &str,
    session_id: &str,
    memory_id: &str,
    role: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO memory_sessions (person_id, session_id, memory_id, role)
         SELECT $1, $2, $3, $4
          WHERE EXISTS (SELECT 1 FROM sessions WHERE person_id = $1 AND session_id = $2)
         ON CONFLICT (memory_id) DO NOTHING",
    )
    .bind(person_id)
    .bind(session_id)
    .bind(memory_id)
    .bind(role)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

impl Store {
    /// One session the viewer may open, with the memories it created and visible `fixes` links.
    pub async fn inspect_session(
        &self,
        viewer: &str,
        owner: &str,
        session_id: &str,
    ) -> Result<SessionInspection> {
        let Some((session, project_id)) = self.visible_session(viewer, owner, session_id).await?
        else {
            return Err(StoreError::NotFound("session".into()));
        };
        let output_visible = visible_with_archive("m", "$3", true);
        let outputs = sqlx::query(&format!(
            "SELECT m.memory_id, m.title, m.kind, m.status, ms.role
               FROM memory_sessions ms
               JOIN memories m ON m.memory_id = ms.memory_id
              WHERE ms.person_id = $1 AND ms.session_id = $2 AND {output_visible}
              ORDER BY ms.created_at, m.memory_id"
        ))
        .bind(owner)
        .bind(session_id)
        .bind(viewer)
        .fetch_all(self.pool())
        .await?;
        let outputs: Vec<SessionOutput> = outputs
            .iter()
            .map(|row| {
                Ok(SessionOutput {
                    id: row.try_get("memory_id")?,
                    title: row.try_get("title")?,
                    kind: row.try_get("kind")?,
                    status: row.try_get("status")?,
                    role: row.try_get("role")?,
                })
            })
            .collect::<Result<_>>()?;
        let ids: Vec<&str> = outputs.iter().map(|output| output.id.as_str()).collect();
        let fix_visible = visible("m", "$2");
        let fixes = sqlx::query(&format!(
            "SELECT DISTINCT m.memory_id, m.title, m.kind, m.status
               FROM links l
               JOIN memories m ON m.memory_id = CASE
                    WHEN l.source_id = ANY($1) THEN l.target_id ELSE l.source_id END
              WHERE l.relation = 'fixes'
                AND (l.source_id = ANY($1) OR l.target_id = ANY($1))
                AND NOT (m.memory_id = ANY($1))
                AND {fix_visible}
              ORDER BY m.title, m.memory_id"
        ))
        .bind(&ids)
        .bind(viewer)
        .fetch_all(self.pool())
        .await?;
        let fixes = fixes
            .iter()
            .map(|row| {
                Ok(SessionOutput {
                    id: row.try_get("memory_id")?,
                    title: row.try_get("title")?,
                    kind: row.try_get("kind")?,
                    status: row.try_get("status")?,
                    role: "fixes".to_owned(),
                })
            })
            .collect::<Result<_>>()?;
        Ok(SessionInspection {
            session,
            project_id,
            outputs,
            fixes,
        })
    }
}
