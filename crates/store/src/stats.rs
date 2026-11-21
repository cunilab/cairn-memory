//! `GET /stats`: every number the dashboard shows, current values only. Counts use the same
//! visibility rule as every other read; nothing here compares with an older version.

use crate::memory::{visible, visible_even_if_expired};
use crate::{Result, Store};
use cairn_core::account::{DeviceHealth, DeviceStatus, device_health};
use cairn_core::id::now_seconds;
use serde::Serialize;
use sqlx::Row;
use std::collections::BTreeMap;

/// Kinds a person expects to be reused: the ones the "never used" and link ratios talk about.
const DURABLE: &str = "('decision','rule','gotcha','procedure','fact')";

/// The benchmark card: the committed v2 run `bench/after-0.1.6-embed-v2.json` (2,000 distractor
/// memories, 43 questions). A unit test keeps these equal to the file.
pub const BENCH: Bench = Bench {
    hit_at_5: 0.6512,
    search_tokens: 133,
    get_tokens: 266,
    session_tokens: 1162,
};

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Bench {
    pub hit_at_5: f64,
    pub search_tokens: i64,
    pub get_tokens: i64,
    pub session_tokens: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Counts {
    pub total: i64,
    pub by_kind: BTreeMap<String, i64>,
    pub by_status: BTreeMap<String, i64>,
    pub by_scope: BTreeMap<String, i64>,
    pub by_origin: BTreeMap<String, i64>,
    /// Expired in the last 30 days and not yet swept away.
    pub expired_30d: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Day {
    pub day: String,
    pub saved: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct UsedMemory {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub fetched: i64,
    pub helpful: i64,
    pub wrong: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct UsageReport {
    /// Durable, active memories no agent has fetched.
    pub never_fetched: i64,
    /// Memories fetched in the last 30 days.
    pub fetched_30d: i64,
    /// Helpful votes in the last 30 days.
    pub helpful_30d: i64,
    pub top: Vec<UsedMemory>,
}

#[derive(Clone, Debug, Serialize)]
pub struct UsageDay {
    pub day: String,
    /// Estimated: response bytes / 4.
    pub tokens: i64,
    pub lookups: i64,
    pub hits: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct UsageDays {
    pub days: Vec<UsageDay>,
    pub total_tokens: i64,
    pub lookups: i64,
    /// Lookups that found something, over all lookups; 0 with none.
    pub hit_ratio: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct StaleCauses {
    pub missing: i64,
    pub feedback: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct StaleReport {
    pub total: i64,
    pub by_cause: StaleCauses,
    pub expiring_30d: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct LinkReport {
    pub total: i64,
    pub auto: i64,
    /// Durable, active memories with at least one visible link, over all of them.
    pub durable_with_link_ratio: f64,
}

/// The dashboard's health card, from the same numbers as `stale` and `links`.
#[derive(Clone, Debug, Serialize)]
pub struct MemoryHealth {
    pub expiring: i64,
    pub stale_missing: i64,
    pub stale_feedback: i64,
    pub linked_ratio: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct JobCounts {
    pub pending: i64,
    pub running: i64,
    pub quarantined: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeviceAttention {
    pub device_id: String,
    pub name: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Capture {
    pub last_at: Option<i64>,
    pub observations_7d: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Stats {
    pub memories: Counts,
    pub activity: Vec<Day>,
    pub usage: UsageReport,
    pub usage_days: UsageDays,
    pub bench: Bench,
    pub stale: StaleReport,
    pub links: LinkReport,
    pub memory_health: MemoryHealth,
    pub jobs: JobCounts,
    pub projects: i64,
    pub devices_checked_in: i64,
    pub devices_attention: Vec<DeviceAttention>,
    /// No device has checked in yet, or nothing has been captured.
    pub first_run: bool,
    pub capture: Capture,
}

pub struct StatsInput<'a> {
    pub person_id: &'a str,
    /// An admin's job counts cover every job.
    pub is_admin: bool,
    /// Minutes east of UTC, -840..=840.
    pub tz_minutes: i32,
    /// 7, 30 or 90.
    pub days: i32,
    pub server_version: &'a str,
    pub public_url: &'a str,
}

fn ratio(part: i64, whole: i64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64 * 10_000.0).round() / 10_000.0
    }
}

impl Store {
    pub async fn stats(&self, input: &StatsInput<'_>) -> Result<Stats> {
        let me = input.person_id;
        let vis = visible("m", "$1");

        let mut counts = Counts {
            total: 0,
            by_kind: BTreeMap::new(),
            by_status: BTreeMap::new(),
            by_scope: BTreeMap::new(),
            by_origin: BTreeMap::new(),
            expired_30d: 0,
        };
        let rows = sqlx::query(&format!(
            "SELECT 'kind' AS dim, m.kind AS k, count(*) AS n FROM memories m WHERE {vis} GROUP BY m.kind
             UNION ALL SELECT 'status', m.status, count(*) FROM memories m WHERE {vis} GROUP BY m.status
             UNION ALL SELECT 'scope', m.scope, count(*) FROM memories m WHERE {vis} GROUP BY m.scope
             UNION ALL SELECT 'origin', m.origin, count(*) FROM memories m WHERE {vis} GROUP BY m.origin"
        ))
        .bind(me)
        .fetch_all(&self.pool)
        .await?;
        for row in rows {
            let (dim, key, n): (String, String, i64) =
                (row.try_get("dim")?, row.try_get("k")?, row.try_get("n")?);
            let map = match dim.as_str() {
                "kind" => &mut counts.by_kind,
                "status" => &mut counts.by_status,
                "scope" => &mut counts.by_scope,
                _ => &mut counts.by_origin,
            };
            map.insert(key, n);
        }
        counts.total = counts.by_status.values().sum();
        counts.expired_30d = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM memories m
              WHERE {} AND m.expires_at <= now() AND m.expires_at > now() - interval '30 days'
                AND NOT (m.origin = 'system' AND m.scope = 'global')",
            visible_even_if_expired("m", "$1")
        ))
        .bind(me)
        .fetch_one(&self.pool)
        .await?;

        // Local days, newest last; days with nothing are zero.
        let local_today = "((now() AT TIME ZONE 'UTC') + make_interval(mins => $2))::date";
        let local_day = |column: &str| {
            format!(
                "to_char(({column} AT TIME ZONE 'UTC') + make_interval(mins => $2), 'YYYY-MM-DD')"
            )
        };
        let activity = sqlx::query(&format!(
            "SELECT d.day, count(m.memory_id) AS saved
               FROM (SELECT to_char(g, 'YYYY-MM-DD') AS day
                       FROM generate_series({local_today} - ($3::int - 1), {local_today}, interval '1 day') g) d
               LEFT JOIN memories m ON {} = d.day AND {vis}
              GROUP BY d.day ORDER BY d.day",
            local_day("m.created_at")
        ))
        .bind(me)
        .bind(input.tz_minutes)
        .bind(input.days)
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(|r| {
            Ok(Day {
                day: r.try_get("day")?,
                saved: r.try_get("saved")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

        let days = sqlx::query(&format!(
            "SELECT d.day, coalesce(sum(r.bytes), 0)::bigint / 4 AS tokens,
                    coalesce(sum(r.calls), 0)::bigint AS lookups, coalesce(sum(r.hits), 0)::bigint AS hits
               FROM (SELECT to_char(g, 'YYYY-MM-DD') AS day
                       FROM generate_series({local_today} - ($3::int - 1), {local_today}, interval '1 day') g) d
               LEFT JOIN retrieval_log r ON r.person_id = $1 AND {} = d.day
              GROUP BY d.day ORDER BY d.day",
            local_day("r.bucket")
        ))
        .bind(me)
        .bind(input.tz_minutes)
        .bind(input.days)
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(|r| {
            Ok(UsageDay {
                day: r.try_get("day")?,
                tokens: r.try_get("tokens")?,
                lookups: r.try_get("lookups")?,
                hits: r.try_get("hits")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
        let (lookups, hits): (i64, i64) = days
            .iter()
            .fold((0, 0), |(l, h), d| (l + d.lookups, h + d.hits));
        let usage_days = UsageDays {
            total_tokens: days.iter().map(|d| d.tokens).sum(),
            hit_ratio: ratio(hits, lookups),
            lookups,
            days,
        };

        let usage_counts = sqlx::query(&format!(
            "SELECT
               (SELECT count(*) FROM memories m LEFT JOIN usage u ON u.memory_id = m.memory_id
                 WHERE {vis} AND m.status = 'active' AND m.kind IN {DURABLE} AND coalesce(u.fetched, 0) = 0) AS never_fetched,
               (SELECT count(*) FROM memories m JOIN usage u ON u.memory_id = m.memory_id
                 WHERE {vis} AND u.last_used_at > now() - interval '30 days') AS fetched_30d,
               (SELECT count(*) FROM usage_votes v JOIN memories m ON m.memory_id = v.memory_id
                 WHERE {vis} AND v.vote = 'helpful' AND v.created_at > now() - interval '30 days') AS helpful_30d"
        ))
        .bind(me)
        .fetch_one(&self.pool)
        .await?;
        let top = sqlx::query(&format!(
            "SELECT m.memory_id, m.kind, m.title, u.fetched::bigint, u.helpful::bigint, u.wrong::bigint
               FROM memories m JOIN usage u ON u.memory_id = m.memory_id
              WHERE {vis} AND u.fetched > 0
              ORDER BY u.fetched DESC, m.memory_id LIMIT 5"
        ))
        .bind(me)
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(|r| {
            Ok(UsedMemory {
                id: r.try_get("memory_id")?,
                kind: r.try_get("kind")?,
                title: r.try_get("title")?,
                fetched: r.try_get("fetched")?,
                helpful: r.try_get("helpful")?,
                wrong: r.try_get("wrong")?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
        let usage = UsageReport {
            never_fetched: usage_counts.try_get("never_fetched")?,
            fetched_30d: usage_counts.try_get("fetched_30d")?,
            helpful_30d: usage_counts.try_get("helpful_30d")?,
            top,
        };

        let stale_row = sqlx::query(&format!(
            "SELECT count(*) FILTER (WHERE m.status = 'stale') AS total,
                    count(*) FILTER (WHERE m.status = 'stale' AND m.stale_cause = 'missing') AS missing,
                    count(*) FILTER (WHERE m.status = 'stale' AND m.stale_cause = 'feedback') AS feedback,
                    count(*) FILTER (WHERE m.expires_at > now() AND m.expires_at <= now() + interval '30 days') AS expiring
               FROM memories m WHERE {vis}"
        ))
        .bind(me)
        .fetch_one(&self.pool)
        .await?;
        let stale = StaleReport {
            total: stale_row.try_get("total")?,
            by_cause: StaleCauses {
                missing: stale_row.try_get("missing")?,
                feedback: stale_row.try_get("feedback")?,
            },
            expiring_30d: stale_row.try_get("expiring")?,
        };

        let link_row = sqlx::query(&format!(
            "SELECT count(*) AS total, count(*) FILTER (WHERE l.origin = 'auto') AS auto
               FROM links l JOIN memories m ON m.memory_id = l.source_id
               JOIN memories t ON t.memory_id = l.target_id
              WHERE {vis} AND {}",
            visible("t", "$1")
        ))
        .bind(me)
        .fetch_one(&self.pool)
        .await?;
        let durable = sqlx::query(&format!(
            "SELECT count(*) AS total,
                    count(*) FILTER (WHERE EXISTS (
                        SELECT 1 FROM links l JOIN memories o
                          ON o.memory_id = CASE WHEN l.source_id = m.memory_id THEN l.target_id ELSE l.source_id END
                         WHERE (l.source_id = m.memory_id OR l.target_id = m.memory_id) AND {})) AS linked
               FROM memories m WHERE {vis} AND m.status = 'active' AND m.kind IN {DURABLE}",
            visible("o", "$1")
        ))
        .bind(me)
        .fetch_one(&self.pool)
        .await?;
        let linked_ratio = ratio(durable.try_get("linked")?, durable.try_get("total")?);
        let links = LinkReport {
            total: link_row.try_get("total")?,
            auto: link_row.try_get("auto")?,
            durable_with_link_ratio: linked_ratio,
        };
        let memory_health = MemoryHealth {
            expiring: stale.expiring_30d,
            stale_missing: stale.by_cause.missing,
            stale_feedback: stale.by_cause.feedback,
            linked_ratio,
        };

        let (pending, running, quarantined) = if input.is_admin {
            let all = self.job_snapshot().await?;
            (all.pending, all.running, all.quarantined)
        } else {
            self.person_job_counts(me).await?
        };
        let projects: i64 =
            sqlx::query_scalar("SELECT count(*) FROM project_members WHERE person_id = $1")
                .bind(me)
                .fetch_one(&self.pool)
                .await?;

        let now = now_seconds();
        let mut devices_checked_in = 0;
        let mut devices_attention = Vec::new();
        for device in self.list_devices(me).await? {
            if device.status != DeviceStatus::Active {
                continue;
            }
            if device.client_version.is_some()
                || device.legacy_hook
                || device.last_seen_at > device.created_at
            {
                devices_checked_in += 1;
            }
            if let Some(report) =
                device_health(&device, input.server_version, input.public_url, now)
                && report.health == DeviceHealth::UpdateNeeded
            {
                devices_attention.push(DeviceAttention {
                    device_id: device.device_id,
                    name: device.name,
                    reason: report.reason.unwrap_or_default(),
                });
            }
        }
        let captured: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM sessions WHERE person_id = $1)")
                .bind(me)
                .fetch_one(&self.pool)
                .await?;
        let capture = sqlx::query(
            "SELECT EXTRACT(EPOCH FROM max(captured_at))::bigint AS last_at,
                    count(*) FILTER (WHERE captured_at > now() - interval '7 days') AS recent
               FROM observations WHERE person_id = $1",
        )
        .bind(me)
        .fetch_one(&self.pool)
        .await?;

        Ok(Stats {
            memories: counts,
            activity,
            usage,
            usage_days,
            bench: BENCH,
            stale,
            links,
            memory_health,
            jobs: JobCounts {
                pending,
                running,
                quarantined,
            },
            projects,
            devices_checked_in,
            devices_attention,
            first_run: devices_checked_in == 0 || !captured,
            capture: Capture {
                last_at: capture.try_get("last_at")?,
                observations_7d: capture.try_get("recent")?,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The card shows the committed benchmark run; this fails when the file moves on.
    #[test]
    fn bench_constants_equal_the_committed_run() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../bench/after-0.1.6-embed-v2.json"
        );
        let file: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("bench file")).unwrap();
        let search = &file["recall"]["memory_search"];
        assert_eq!(search["hit_at_5"].as_f64(), Some(BENCH.hit_at_5));
        assert_eq!(
            search["bytes_per_call"]["mean_tokens_est"].as_i64(),
            Some(BENCH.search_tokens)
        );
        assert_eq!(
            file["call_bytes"]["memory_get"]["mean_tokens_est"].as_i64(),
            Some(BENCH.get_tokens)
        );
        assert_eq!(
            file["fixed_session_cost"]["eager_hosts_tokens_est"].as_i64(),
            Some(BENCH.session_tokens)
        );
    }
}
