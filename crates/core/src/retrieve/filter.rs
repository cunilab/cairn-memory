//! Request-side narrowing for agent search.

use crate::capture::WorkContext;
use crate::memory::{Kind, Memory, Scope};
use serde::Deserialize;

/// What the caller is working on, and the kind it asks for. With no field set, nothing is
/// narrowed.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Filter {
    #[serde(rename = "sessionId", alias = "session_id", default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub context: Option<WorkContext>,
    #[serde(default)]
    pub kind: Option<Kind>,
    /// Inclusive unix seconds. Set only by an explicit search argument.
    #[serde(default)]
    pub recorded_from: Option<i64>,
    /// Inclusive unix seconds.
    #[serde(default)]
    pub recorded_to: Option<i64>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

/// `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SSZ`. A date alone is the start of that UTC day,
/// or the last second of that day when `end` is set.
pub fn parse_recorded(value: &str, end: bool) -> Result<i64, String> {
    let value = value.trim();
    let (date, clock) = value
        .split_once('T')
        .map(|(date, clock)| (date, Some(clock.trim_end_matches('Z'))))
        .unwrap_or((value, None));
    let mut parts = date.split('-');
    let year: i32 = parts
        .next()
        .and_then(|part| part.parse().ok())
        .filter(|year| (1970..=2100).contains(year))
        .ok_or_else(|| "recorded time must be an ISO date".to_owned())?;
    let month: u32 = parts
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or_else(|| "recorded time must be an ISO date".to_owned())?;
    let day: u32 = parts
        .next()
        .and_then(|part| part.parse().ok())
        .ok_or_else(|| "recorded time must be an ISO date".to_owned())?;
    if parts.next().is_some() || day == 0 || day > days_in_month(year, month) {
        return Err("recorded time must be an ISO date".into());
    }
    let mut seconds = ymd_to_unix(year, month, day).ok_or("recorded time must be an ISO date")?;
    if let Some(clock) = clock {
        let mut time = clock.split(':');
        let hour: u32 = time
            .next()
            .and_then(|part| part.parse().ok())
            .ok_or("recorded time must be an ISO date")?;
        let minute: u32 = time
            .next()
            .and_then(|part| part.parse().ok())
            .ok_or("recorded time must be an ISO date")?;
        let second: u32 = time
            .next()
            .and_then(|part| part.split('.').next())
            .and_then(|part| part.parse().ok())
            .ok_or("recorded time must be an ISO date")?;
        if time.next().is_some() || hour > 23 || minute > 59 || second > 59 {
            return Err("recorded time must be an ISO date".into());
        }
        seconds += i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second);
    } else if end {
        seconds += 86_400 - 1;
    }
    Ok(seconds)
}

fn leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn ymd_to_unix(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
        return None;
    }
    let mut days = 0_i64;
    for y in 1970..year {
        days += if leap(y) { 366 } else { 365 };
    }
    for m in 1..month {
        days += i64::from(days_in_month(year, m));
    }
    Some((days + i64::from(day) - 1) * 86_400)
}

/// Explicit recorded-at, branch and version selectors. Unset selectors do not narrow.
pub fn within_selectors(memory: &Memory, filter: &Filter) -> bool {
    if filter
        .recorded_from
        .is_some_and(|from| memory.created_at < from)
        || filter.recorded_to.is_some_and(|to| memory.created_at > to)
    {
        return false;
    }
    if let Some(branch) = &filter.branch
        && memory.context.branch.as_deref() != Some(branch.as_str())
    {
        return false;
    }
    if let Some(version) = &filter.version {
        let blob = format!(
            "{} {} {} {}",
            memory.title,
            memory.summary,
            memory.reason,
            memory.fields.to_json()
        )
        .to_lowercase();
        if !blob.contains(&version.to_lowercase()) {
            return false;
        }
    }
    true
}

/// A memory of another kind never matches. Global memories always match. A project memory matches the context's project. A handoff
/// matches the repository, branch and worktree it was written for; other personal memories match.
pub fn matches(memory: &Memory, filter: &Filter) -> bool {
    if filter.kind.is_some_and(|kind| kind != memory.kind) {
        return false;
    }
    if filter.session_id.is_none() && filter.context.is_none() {
        return true;
    }
    match memory.scope {
        Scope::Global => true,
        Scope::Project => filter
            .context
            .as_ref()
            .and_then(|c| c.project_id.as_ref())
            .is_some_and(|id| memory.project_id.as_ref() == Some(id)),
        Scope::Personal => {
            if memory.kind != Kind::Handoff {
                return true;
            }
            filter.context.as_ref().is_none_or(|c| {
                c.repository
                    .as_ref()
                    .is_none_or(|r| memory.context.repo.as_ref() == Some(r))
                    && c.branch
                        .as_ref()
                        .is_none_or(|b| memory.context.branch.as_ref() == Some(b))
                    && c.worktree
                        .as_ref()
                        .is_none_or(|w| memory.context.worktree.as_ref() == Some(w))
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::memory;

    #[test]
    fn project_memories_need_the_context_project() {
        let m = memory("a", Kind::Rule, Scope::Project);
        assert!(matches(&m, &Filter::default()));
        let other = Filter {
            context: Some(WorkContext {
                project_id: Some("p2".into()),
                ..WorkContext::default()
            }),
            ..Filter::default()
        };
        assert!(!matches(&m, &other));
        let own = Filter {
            context: Some(WorkContext {
                project_id: Some("p1".into()),
                ..WorkContext::default()
            }),
            ..Filter::default()
        };
        assert!(matches(&m, &own));
    }

    #[test]
    fn kind_filter_narrows_to_that_kind() {
        let m = memory("a", Kind::Rule, Scope::Global);
        let want = |kind| Filter {
            kind: Some(kind),
            ..Filter::default()
        };
        assert!(matches(&m, &want(Kind::Rule)));
        assert!(!matches(&m, &want(Kind::Decision)));
    }

    #[test]
    fn handoffs_match_their_checkout() {
        let mut h = memory("h", Kind::Handoff, Scope::Personal);
        h.context.branch = Some("main".into());
        let on = |b: &str| Filter {
            context: Some(WorkContext {
                branch: Some(b.into()),
                ..WorkContext::default()
            }),
            ..Filter::default()
        };
        assert!(matches(&h, &on("main")));
        assert!(!matches(&h, &on("other")));
    }

    #[test]
    fn recorded_dates_are_utc_and_a_bare_day_spans_that_day() {
        assert_eq!(parse_recorded("2026-01-15", false).unwrap(), 1768435200);
        assert_eq!(
            parse_recorded("2026-01-15", true).unwrap(),
            1768435200 + 86_400 - 1
        );
        assert_eq!(
            parse_recorded("2026-01-15T00:00:00Z", false).unwrap(),
            1768435200
        );
        assert!(parse_recorded("yesterday", false).is_err());
        assert!(parse_recorded("2026-02-31", false).is_err());
    }

    #[test]
    fn selectors_drop_other_days_branches_and_versions() {
        let mut memory = memory("a", Kind::Fact, Scope::Project);
        memory.created_at = parse_recorded("2026-01-15T00:00:00Z", false).unwrap();
        memory.context.branch = Some("feature/amber".into());
        memory.summary = "Scripts call harbor-cli 1.4.2".into();
        let on_day = Filter {
            recorded_from: parse_recorded("2026-01-15", false).ok(),
            recorded_to: parse_recorded("2026-01-15", true).ok(),
            branch: Some("feature/amber".into()),
            version: Some("1.4.2".into()),
            ..Filter::default()
        };
        assert!(within_selectors(&memory, &on_day));
        let other_day = Filter {
            recorded_from: parse_recorded("2026-06-01", false).ok(),
            recorded_to: parse_recorded("2026-06-01", true).ok(),
            ..Filter::default()
        };
        assert!(!within_selectors(&memory, &other_day));
        assert!(!within_selectors(
            &memory,
            &Filter {
                branch: Some("feature/birch".into()),
                ..Filter::default()
            }
        ));
        assert!(!within_selectors(
            &memory,
            &Filter {
                version: Some("9.9.9".into()),
                ..Filter::default()
            }
        ));
    }
}
