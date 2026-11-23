use crate::{Result, Store};

impl Store {
    pub async fn record_lookup(
        &self,
        identity: (&str, &str),
        session_id: &str,
        kind: &str,
        query: Option<&str>,
        ids: &[String],
        hit: Option<bool>,
    ) -> Result<()> {
        sqlx::query("INSERT INTO lookup_log (person_id,device_id,session_id,kind,query,result_ids,hit) VALUES ($1,$2,$3,$4,$5,$6,$7)")
            .bind(identity.0).bind(identity.1).bind(session_id).bind(kind).bind(query).bind(ids).bind(hit)
            .execute(&self.pool).await?;
        Ok(())
    }
}

use cairn_core::fixes::{self, Fix};
use cairn_core::id::new_id;
use sqlx::{Postgres, Row, Transaction};

struct Candidate {
    detector: &'static str,
    kind: &'static str,
    subject_kind: &'static str,
    subject_id: Option<String>,
    person_id: Option<String>,
    project_id: Option<String>,
    device_id: Option<String>,
    data: Fix,
}

impl Store {
    pub async fn fixes_due(&self) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT COALESCE((SELECT value <> CURRENT_DATE::text FROM server_state WHERE key='fixes_last_run'), true)")
            .fetch_one(&self.pool).await?)
    }

    /// Runs a single daily detector pass. All detector and lifecycle updates commit together.
    pub async fn fixes_nightly(&self, semantic: bool, version: &str) -> Result<()> {
        self.fixes_nightly_with_queries(semantic, version, &[])
            .await
    }

    /// Search phrases eligible for F11, to embed using the server's current local model.
    pub async fn fixes_alias_queries(&self) -> Result<Vec<String>> {
        if !crate::retrieve::ALIASES_IN_SEARCH {
            return Ok(Vec::new());
        }
        Ok(sqlx::query_scalar("SELECT DISTINCT query FROM lookup_log WHERE kind='search' AND hit=false AND query IS NOT NULL AND EXISTS (SELECT 1 FROM lookup_log WHERE at <= now()-interval '14 days')")
            .fetch_all(&self.pool).await?)
    }

    pub async fn fixes_nightly_with_queries(
        &self,
        semantic: bool,
        version: &str,
        vectors: &[(String, Vec<f32>)],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("INSERT INTO server_state (key,value) VALUES ('fixes_last_run','') ON CONFLICT DO NOTHING")
            .execute(&mut *tx).await?;
        let already: bool = sqlx::query_scalar("SELECT value = CURRENT_DATE::text FROM server_state WHERE key='fixes_last_run' FOR UPDATE")
            .fetch_one(&mut *tx).await?;
        if already {
            return Ok(());
        }
        let mut candidates = Vec::new();
        let rows = sqlx::query("SELECT m.memory_id,m.owner_id,m.project_id,m.stale_cause, floor(EXTRACT(EPOCH FROM now()-m.status_changed_at)/86400)::bigint AS days, u.last_used_at > m.status_changed_at AS fetched, m.checked_at > m.status_changed_at AS checked FROM memories m LEFT JOIN usage u USING(memory_id) WHERE m.status='stale' AND m.scope <> 'global' AND m.origin <> 'system' AND m.stale_cause IN ('feedback','missing')")
            .fetch_all(&mut *tx).await?;
        for row in rows {
            let id: String = row.try_get("memory_id")?;
            let cause: String = row.try_get("stale_cause")?;
            let days: i64 = row.try_get("days")?;
            let (detector, data) = if cause == "feedback" {
                (
                    "F2",
                    fixes::f2(
                        &id,
                        days,
                        row.try_get::<Option<bool>, _>("fetched")?.unwrap_or(false),
                    ),
                )
            } else {
                (
                    "F3",
                    fixes::f3(
                        &id,
                        days,
                        row.try_get::<Option<bool>, _>("checked")?.unwrap_or(false),
                    ),
                )
            };
            if let Some(data) = data {
                candidates.push(Candidate {
                    detector,
                    kind: "agent",
                    subject_kind: "memory",
                    subject_id: Some(id),
                    person_id: row.try_get("owner_id")?,
                    project_id: row.try_get("project_id")?,
                    device_id: None,
                    data,
                });
            }
        }
        let rows = sqlx::query("SELECT device_id, person_id, client_version, hook_error_class, last_hook_at < now()-interval '7 days' AS hook_old, last_seen_at > now()-interval '2 days' AS seen_recent FROM devices WHERE status='active'").fetch_all(&mut *tx).await?;
        for row in rows {
            let id: String = row.try_get("device_id")?;
            let person: String = row.try_get("person_id")?;
            if let Some(mut data) = fixes::f4(
                &id,
                if row.try_get::<Option<bool>, _>("hook_old")?.unwrap_or(false) {
                    8
                } else {
                    7
                },
                if row.try_get::<bool, _>("seen_recent")? {
                    1
                } else {
                    2
                },
                true,
            ) {
                data.2 = serde_json::json!({"hook_error_class":row.try_get::<Option<String>,_>("hook_error_class")?});
                candidates.push(Candidate {
                    detector: "F4",
                    kind: "computer",
                    subject_kind: "device",
                    subject_id: Some(id.clone()),
                    person_id: Some(person.clone()),
                    project_id: None,
                    device_id: Some(id.clone()),
                    data,
                });
            }
            if let Some(client) = row.try_get::<Option<String>, _>("client_version")?
                && let Some(data) = fixes::f5(&id, &client, version)
            {
                candidates.push(Candidate {
                    detector: "F5",
                    kind: "computer",
                    subject_kind: "device",
                    subject_id: Some(id.clone()),
                    person_id: Some(person),
                    project_id: None,
                    device_id: Some(id),
                    data,
                });
            }
        }
        let row = sqlx::query("SELECT count(*) FILTER (WHERE embedding IS NULL AND created_at < now()-interval '1 hour')::bigint AS missing, count(*) FILTER (WHERE embedding IS NOT NULL AND embedding_model IS DISTINCT FROM $1)::bigint AS foreign_count FROM memories WHERE status='active' AND scope <> 'global' AND origin <> 'system'")
            .bind(cairn_core::process::embed::model_label()).fetch_one(&mut *tx).await?;
        if let Some(data) = fixes::f8(
            semantic,
            row.try_get::<i64, _>("missing")? as u64,
            row.try_get::<i64, _>("foreign_count")? as u64,
        ) {
            candidates.push(Candidate {
                detector: "F8",
                kind: "report",
                subject_kind: "server",
                subject_id: None,
                person_id: None,
                project_id: None,
                device_id: None,
                data,
            });
        }
        let rows = sqlx::query("SELECT memory_id,owner_id,project_id,title,summary,reason,fields::text AS fields FROM memories WHERE status IN ('active','stale') AND scope <> 'global' AND origin <> 'system'").fetch_all(&mut *tx).await?;
        for row in rows {
            let id: String = row.try_get("memory_id")?;
            for field in ["title", "summary", "reason", "fields"] {
                let text: String = row.try_get(field)?;
                if let Some(data) = fixes::f9(&id, field, &text) {
                    candidates.push(Candidate {
                        detector: "F9",
                        kind: "server",
                        subject_kind: "memory",
                        subject_id: Some(id.clone()),
                        person_id: row.try_get("owner_id")?,
                        project_id: row.try_get("project_id")?,
                        device_id: None,
                        data,
                    });
                }
            }
        }
        let rows = sqlx::query("SELECT s.project_id,p.owner_id,count(*)::bigint AS sessions FROM sessions s JOIN projects p ON p.project_id=s.project_id WHERE s.started_at >= now()-interval '7 days' AND s.started_at >= (SELECT min(started_at)+interval '7 days' FROM sessions WHERE nudge_asked IS NOT NULL) AND coalesce(s.context->>'origin','') <> 'automation' AND s.notes_written=0 AND s.nudge_saved=false AND EXISTS (SELECT 1 FROM observations o WHERE o.person_id=s.person_id AND o.session_id=s.session_id AND o.kind IN ('edit','command')) AND NOT EXISTS (SELECT 1 FROM memories m WHERE m.project_id=s.project_id AND m.origin='agent' AND m.created_at >= now()-interval '7 days') GROUP BY s.project_id,p.owner_id")
            .fetch_all(&mut *tx).await?;
        for row in rows {
            let id: String = row.try_get("project_id")?;
            if let Some(data) = fixes::f1(&id, row.try_get::<i64, _>("sessions")? as u32, 0, true) {
                candidates.push(Candidate {
                    detector: "F1",
                    kind: "agent",
                    subject_kind: "project",
                    subject_id: Some(id.clone()),
                    person_id: row.try_get("owner_id")?,
                    project_id: Some(id),
                    device_id: None,
                    data,
                });
            }
        }
        let rows = sqlx::query("SELECT s.person_id,s.context->>'repository' AS repo,count(DISTINCT s.session_id)::bigint AS sessions, array_agg(DISTINCT p.project_id ORDER BY p.project_id) AS projects FROM sessions s JOIN projects p ON p.repository=s.context->>'repository' WHERE s.project_id IS NULL AND s.started_at >= now()-interval '7 days' AND s.context->>'repository' NOT LIKE 'local:%' GROUP BY s.person_id,s.context->>'repository'")
            .fetch_all(&mut *tx).await?;
        for row in rows {
            let person: String = row.try_get("person_id")?;
            let repo: String = row.try_get("repo")?;
            let projects: Vec<String> = row.try_get("projects")?;
            if projects.len() > 1 {
                let subject = cairn_core::id::sha256_hex(&[&person, &repo]);
                if let Some(data) = fixes::f6(
                    &subject,
                    row.try_get::<i64, _>("sessions")? as u32,
                    &projects,
                    None,
                ) {
                    candidates.push(Candidate {
                        detector: "F6",
                        kind: "report",
                        subject_kind: "repository",
                        subject_id: Some(subject),
                        person_id: Some(person),
                        project_id: None,
                        device_id: None,
                        data,
                    });
                }
            } else if let Some(project) = projects.first() {
                let name: String =
                    sqlx::query_scalar("SELECT name FROM projects WHERE project_id=$1")
                        .bind(project)
                        .fetch_one(&mut *tx)
                        .await?;
                let devices = sqlx::query("SELECT device_id,count(*)::bigint AS sessions FROM sessions WHERE person_id=$1 AND context->>'repository'=$2 AND project_id IS NULL AND started_at >= now()-interval '7 days' AND device_id IS NOT NULL GROUP BY device_id")
                    .bind(&person).bind(&repo).fetch_all(&mut *tx).await?;
                for device in devices {
                    let device_id: String = device.try_get("device_id")?;
                    let subject = cairn_core::id::sha256_hex(&[&person, &repo, &device_id]);
                    if let Some(data) = fixes::f6(
                        &subject,
                        device.try_get::<i64, _>("sessions")? as u32,
                        &projects,
                        Some((project, &repo, &name)),
                    ) {
                        candidates.push(Candidate {
                            detector: "F6",
                            kind: "computer",
                            subject_kind: "repository",
                            subject_id: Some(subject),
                            person_id: Some(person.clone()),
                            project_id: Some(project.clone()),
                            device_id: Some(device_id),
                            data,
                        });
                    }
                }
            }
        }
        let rows = sqlx::query("SELECT l.source_id,l.target_id,a.owner_id,a.project_id,a.title||' '||a.summary AS newer,b.title||' '||b.summary AS older FROM links l JOIN memories a ON a.memory_id=l.source_id JOIN memories b ON b.memory_id=l.target_id WHERE l.relation='supersedes' AND l.origin='auto' AND a.status IN ('active','stale') AND a.scope <> 'global' AND a.origin <> 'system' AND b.scope <> 'global' AND b.origin <> 'system'")
            .fetch_all(&mut *tx).await?;
        for row in rows {
            let source: String = row.try_get("source_id")?;
            let target: String = row.try_get("target_id")?;
            if let Some(data) = fixes::f7(
                &source,
                &target,
                &row.try_get::<String, _>("newer")?,
                &row.try_get::<String, _>("older")?,
            ) {
                candidates.push(Candidate {
                    detector: "F7",
                    kind: "server",
                    subject_kind: "memory",
                    subject_id: Some(source),
                    person_id: row.try_get("owner_id")?,
                    project_id: row.try_get("project_id")?,
                    device_id: None,
                    data,
                });
            }
        }
        let rows = sqlx::query("SELECT s.project_id,p.owner_id,count(*)::bigint AS shown,count(*) FILTER (WHERE s.nudge_saved=true)::bigint AS saved FROM sessions s JOIN projects p ON p.project_id=s.project_id WHERE s.started_at >= now()-interval '14 days' AND s.nudge_asked=true AND EXISTS (SELECT 1 FROM sessions earliest WHERE earliest.nudge_asked=true AND earliest.started_at <= now()-interval '14 days') GROUP BY s.project_id,p.owner_id")
            .fetch_all(&mut *tx).await?;
        for row in rows {
            let id: String = row.try_get("project_id")?;
            if let Some(data) = fixes::f10(
                &id,
                14,
                row.try_get::<i64, _>("shown")? as u32,
                row.try_get::<i64, _>("saved")? as u32,
            ) {
                candidates.push(Candidate {
                    detector: "F10",
                    kind: "server",
                    subject_kind: "project",
                    subject_id: Some(id.clone()),
                    person_id: row.try_get("owner_id")?,
                    project_id: Some(id),
                    device_id: None,
                    data,
                });
            }
        }
        let rows = sqlx::query("SELECT m.memory_id,m.owner_id,m.project_id,count(l.id)::bigint AS searches FROM memories m LEFT JOIN usage u USING(memory_id) JOIN sessions s ON s.project_id=m.project_id JOIN lookup_log l ON l.person_id=s.person_id AND l.session_id=s.session_id AND l.kind='search' AND l.at >= now()-interval '90 days' WHERE m.status IN ('active','stale') AND m.scope <> 'global' AND m.origin <> 'system' AND m.kind NOT IN ('decision','rule','procedure','gotcha','preference','handoff') AND m.faded_at IS NULL AND m.created_at < now()-interval '90 days' AND (u.last_used_at IS NULL OR u.last_used_at <= now()-interval '90 days') GROUP BY m.memory_id,m.owner_id,m.project_id")
            .fetch_all(&mut *tx).await?;
        for row in rows {
            let id: String = row.try_get("memory_id")?;
            if let Some(data) =
                fixes::f12(&id, 91, 90, row.try_get::<i64, _>("searches")? as u32, true)
            {
                candidates.push(Candidate {
                    detector: "F12",
                    kind: "server",
                    subject_kind: "memory",
                    subject_id: Some(id),
                    person_id: row.try_get("owner_id")?,
                    project_id: row.try_get("project_id")?,
                    device_id: None,
                    data,
                });
            }
        }
        if crate::retrieve::ALIASES_IN_SEARCH {
            for (phrase, vector) in vectors {
                cairn_core::process::embed::validate_vector(vector)?;
                let rows = sqlx::query("SELECT m.memory_id,m.owner_id,m.project_id,count(DISTINCT (a.person_id,a.session_id))::bigint AS sessions, (SELECT count(*) FROM memory_aliases x WHERE x.memory_id=m.memory_id)::bigint AS aliases, (1-(m.embedding <=> $2::vector))::float4 AS cosine FROM lookup_log a JOIN lookup_log b ON b.person_id=a.person_id AND b.session_id=a.session_id AND b.kind='search' AND b.at>a.at AND b.hit=true JOIN lookup_log g ON g.person_id=b.person_id AND g.session_id=b.session_id AND g.kind='get' AND g.at>b.at AND g.hit=true JOIN memories m ON m.memory_id=ANY(b.result_ids) AND m.memory_id=ANY(g.result_ids) WHERE a.kind='search' AND a.hit=false AND a.query=$1 AND m.scope <> 'global' AND m.origin <> 'system' AND (m.owner_id=a.person_id OR (m.scope='project' AND EXISTS (SELECT 1 FROM project_members pm WHERE pm.project_id=m.project_id AND pm.person_id=a.person_id))) AND m.status IN ('active','stale') AND m.embedding IS NOT NULL AND m.embedding_model=$3 AND EXISTS (SELECT 1 FROM lookup_log WHERE at <= now()-interval '14 days') GROUP BY m.memory_id,m.owner_id,m.project_id,m.embedding HAVING count(DISTINCT (a.person_id,a.session_id)) >= 2")
                    .bind(phrase).bind(crate::embed::vector_text(vector)).bind(cairn_core::process::embed::model_label()).fetch_all(&mut *tx).await?;
                for row in rows {
                    let id: String = row.try_get("memory_id")?;
                    if let Some(data) = fixes::f11(
                        (&id, phrase),
                        14,
                        row.try_get::<i64, _>("sessions")? as u32,
                        row.try_get("cosine")?,
                        row.try_get::<i64, _>("aliases")? as u32,
                        true,
                    ) {
                        candidates.push(Candidate {
                            detector: "F11",
                            kind: "server",
                            subject_kind: "memory",
                            subject_id: Some(id),
                            person_id: row.try_get("owner_id")?,
                            project_id: row.try_get("project_id")?,
                            device_id: None,
                            data,
                        });
                    }
                }
            }
        }
        settle_proof(&mut tx).await?;
        persist_candidates(&mut tx, &candidates, semantic).await?;
        sqlx::query("UPDATE server_state SET value=CURRENT_DATE::text WHERE key='fixes_last_run'")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

async fn persist_candidates(
    tx: &mut Transaction<'_, Postgres>,
    candidates: &[Candidate],
    semantic: bool,
) -> Result<()> {
    let fingerprints: Vec<String> = candidates.iter().map(|c| c.data.0.clone()).collect();
    sqlx::query("UPDATE fixes SET quiet_seen_at=now(), status=CASE WHEN status='pending' THEN 'resolved' ELSE status END WHERE status IN ('pending','ignored','applying','queued','waiting') AND NOT (fingerprint=ANY($1)) AND (detector <> 'F11' OR $2 OR NOT $3)")
        .bind(&fingerprints).bind(semantic).bind(crate::retrieve::ALIASES_IN_SEARCH).execute(&mut **tx).await?;
    for c in candidates {
        let existing = sqlx::query("SELECT fix_id,status,quiet_seen_at,ignored_until FROM fixes WHERE detector=$1 AND fingerprint=$2 ORDER BY first_seen_at DESC LIMIT 1 FOR UPDATE")
            .bind(c.detector).bind(&c.data.0).fetch_optional(&mut **tx).await?;
        let mut insert = true;
        if let Some(row) = existing {
            let id: String = row.try_get("fix_id")?;
            let status: String = row.try_get("status")?;
            if ["pending", "applying", "queued", "waiting"].contains(&status.as_str()) {
                sqlx::query("UPDATE fixes SET count=count+1,last_seen_at=now(),quiet_seen_at=NULL,evidence=$2,action=$3 WHERE fix_id=$1")
                    .bind(id).bind(&c.data.2).bind((!c.data.3.is_null()).then_some(&c.data.3)).execute(&mut **tx).await?;
                insert = false;
            } else if status == "ignored" {
                let may_reopen: bool = sqlx::query_scalar("SELECT quiet_seen_at IS NOT NULL AND ignored_until <= now() FROM fixes WHERE fix_id=$1").bind(&id).fetch_one(&mut **tx).await?;
                if !may_reopen {
                    insert = false;
                }
            }
        }
        if insert {
            sqlx::query("INSERT INTO fixes (fix_id,detector,fingerprint,subject_kind,subject_id,project_id,device_id,person_id,fix_type,status,first_seen_at,last_seen_at,evidence,action) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'pending',now(),now(),$10,$11)")
                .bind(new_id("fx")).bind(c.detector).bind(&c.data.0).bind(c.subject_kind).bind(&c.subject_id).bind(&c.project_id).bind(&c.device_id).bind(&c.person_id).bind(c.kind).bind(&c.data.2).bind((!c.data.3.is_null()).then_some(&c.data.3)).execute(&mut **tx).await?;
        }
    }
    Ok(())
}

async fn settle_proof(tx: &mut Transaction<'_, Postgres>) -> Result<()> {
    sqlx::query("UPDATE fixes SET status='expired' WHERE (status='waiting' AND waiting_since < now()-interval '14 days') OR (status='queued' AND applied_at < now()-interval '7 days')")
        .execute(&mut **tx).await?;
    let waiting = sqlx::query("SELECT fix_id,detector,subject_id,project_id FROM fixes WHERE status='waiting' AND fix_type='agent' FOR UPDATE")
        .fetch_all(&mut **tx).await?;
    for row in waiting {
        let fix_id: String = row.try_get("fix_id")?;
        let detector: String = row.try_get("detector")?;
        let project: Option<String> = row.try_get("project_id")?;
        let subject: Option<String> = row.try_get("subject_id")?;
        let proofs: Vec<String> = if detector == "F1" {
            sqlx::query_scalar("SELECT m.memory_id FROM memories m JOIN fixes f ON f.fix_id=$1 WHERE m.project_id=$2 AND m.origin='agent' AND m.status <> 'forgotten' AND m.created_at > f.waiting_since")
                .bind(&fix_id).bind(&project).fetch_all(&mut **tx).await?
        } else {
            sqlx::query_scalar("SELECT m.memory_id FROM memories m JOIN links l ON l.source_id=m.memory_id AND l.relation='supersedes' AND l.origin='agent' JOIN memories old ON old.memory_id=l.target_id JOIN fixes f ON f.fix_id=$1 WHERE l.target_id=$2 AND m.project_id IS NOT DISTINCT FROM $3 AND (old.project_id IS NOT NULL OR m.owner_id=old.owner_id) AND m.origin='agent' AND m.status <> 'forgotten' AND m.identity_hash <> old.identity_hash AND m.created_at > f.waiting_since")
                .bind(&fix_id).bind(&subject).bind(&project).fetch_all(&mut **tx).await?
        };
        let proof = if detector == "F1" || proofs.len() == 1 {
            proofs.first()
        } else {
            None
        };
        if let Some(proof) = proof {
            sqlx::query("UPDATE fixes SET status='done',done_at=now(),proof_id=$2 WHERE fix_id=$1")
                .bind(fix_id)
                .bind(proof)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(())
}
