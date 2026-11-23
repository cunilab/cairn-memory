//! Pure detector thresholds. Storage supplies already-scoped rows and their stable subjects.
//!
//! Action JSON uses `type` plus these fields: `write_project_file` has `project_id`,
//! `repository`, and `name`; `run_update` has `target`; `suggest_setup` has `device_id`;
//! `split_merge` has `source_id` and `target_id`; `redact_secret` has `memory_id` and `fields`;
//! `nudge_off` has `project_id`; `add_alias` has `memory_id` and redacted `phrase`; `fade` has
//! `memory_id`; `agent_prompt` has `memory_id` or `project_id`. Report actions are null.
use crate::account::version_older;
use crate::capture::redact_sensitive_content;
use crate::id::sha256_hex;
use crate::process::consolidate::rare_tokens_differ;
use serde_json::{Value, json};

pub type Fix = (String, String, Value, Value);

fn fix(detector: &str, subject: &str, stable: &str, evidence: Value, action: Value) -> Fix {
    (
        sha256_hex(&[detector, subject, stable]),
        subject.to_owned(),
        evidence,
        action,
    )
}

pub fn f1(subject: &str, sessions: u32, saves: u32, gate: bool) -> Option<Fix> {
    // shortcut: the per-session save flag is hook-observed only; record all saves by session if false alarms appear.
    (gate && sessions >= 3 && saves == 0).then(|| {
        fix(
            "F1",
            subject,
            "",
            json!({"sessions": sessions}),
            json!({"type":"agent_prompt","project_id":subject}),
        )
    })
}
pub fn f2(subject: &str, stale_days: i64, fetched_after: bool) -> Option<Fix> {
    (stale_days >= 7 && fetched_after).then(|| {
        fix(
            "F2",
            subject,
            "feedback",
            json!({"cause":"feedback"}),
            json!({"type":"agent_prompt","memory_id":subject}),
        )
    })
}
pub fn f3(subject: &str, stale_days: i64, later_check: bool) -> Option<Fix> {
    // shortcut: an inconclusive later check counts as still missing; record per-check outcome if F3 false alarms show up.
    (stale_days >= 7 && later_check).then(|| {
        fix(
            "F3",
            subject,
            "missing",
            json!({"cause":"missing"}),
            json!({"type":"agent_prompt","memory_id":subject}),
        )
    })
}
pub fn f4(subject: &str, hook_days: i64, seen_days: i64, active: bool) -> Option<Fix> {
    (active && hook_days > 7 && seen_days < 2).then(|| {
        fix(
            "F4",
            subject,
            "",
            json!({}),
            json!({"type":"suggest_setup","device_id":subject}),
        )
    })
}
pub fn f5(subject: &str, version: &str, target: &str) -> Option<Fix> {
    version_older(version, target).then(|| {
        fix(
            "F5",
            subject,
            target,
            json!({}),
            json!({"type":"run_update","target":target}),
        )
    })
}
pub fn f6(
    subject: &str,
    sessions: u32,
    projects: &[String],
    target: Option<(&str, &str, &str)>,
) -> Option<Fix> {
    if sessions < 2 || projects.is_empty() {
        return None;
    }
    let action = match (projects.len(), target) {
        (1, Some((project_id, repository, name))) => {
            json!({"type":"write_project_file","project_id":project_id,"repository":repository,"name":name})
        }
        (n, _) if n > 1 => Value::Null,
        _ => return None,
    };
    let stable = sha256_hex(&[&projects.join("\0"), target.map_or("", |(_, _, name)| name)]);
    Some(fix(
        "F6",
        subject,
        &stable,
        json!({"sessions":sessions,"projects":projects.len()}),
        action,
    ))
}
pub fn f7(source: &str, target: &str, newer: &str, older: &str) -> Option<Fix> {
    rare_tokens_differ(newer, older).then(|| {
        fix(
            "F7",
            source,
            &sha256_hex(&[target, newer, older]),
            json!({}),
            json!({"type":"split_merge","source_id":source,"target_id":target}),
        )
    })
}
pub fn f8(semantic: bool, unembedded: u64, foreign: u64) -> Option<Fix> {
    let stable = format!("{}:{}:{}", !semantic, unembedded > 0, foreign > 0);
    (!semantic || unembedded > 0 || foreign > 0).then(|| fix("F8", "server", &stable, json!({"semantic_off":!semantic,"unembedded_over_1h":unembedded,"foreign_model":foreign}), Value::Null))
}
pub fn f9(subject: &str, field: &str, text: &str) -> Option<Fix> {
    (redact_sensitive_content(text) != text).then(|| {
        fix(
            "F9",
            subject,
            &sha256_hex(&[field, text]),
            json!({"class":"secret","field":field}),
            json!({"type":"redact_secret","memory_id":subject,"fields":[field]}),
        )
    })
}
pub fn f10(subject: &str, days: i64, shown: u32, saved: u32) -> Option<Fix> {
    (days >= 14 && shown >= 10 && saved <= 1).then(|| {
        fix(
            "F10",
            subject,
            "",
            json!({"shown":shown,"saved":saved}),
            json!({"type":"nudge_off","project_id":subject}),
        )
    })
}
pub fn f11(
    pair: (&str, &str),
    days: i64,
    sessions: u32,
    cosine: f32,
    aliases: u32,
    live: bool,
) -> Option<Fix> {
    (live && days >= 14 && sessions >= 2 && cosine >= 0.43 && aliases < 5).then(|| {
        fix(
            "F11",
            pair.0,
            &sha256_hex(&[pair.1]),
            json!({"sessions":sessions}),
            json!({"type":"add_alias","memory_id":pair.0,"phrase":pair.1}),
        )
    })
}
pub fn f12(
    subject: &str,
    age_days: i64,
    unused_days: i64,
    searches: u32,
    allowed: bool,
) -> Option<Fix> {
    (allowed && age_days > 90 && unused_days >= 90 && searches >= 20).then(|| {
        fix(
            "F12",
            subject,
            "",
            json!({"searches":searches}),
            json!({"type":"fade","memory_id":subject}),
        )
    })
}

#[cfg(test)]
mod tests;
