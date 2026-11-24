use super::entities;
use crate::capture::{WorkContext, redact_sensitive_content};
use crate::error::Error;
use crate::memory::identity::{evidence_hash, identity_hash, normalize_evidence};
use crate::memory::kind::{expires_at, resolve_scope};
use crate::memory::validate::{validate_quality, validate_save};
use crate::memory::{Context, Entity, Fields, Kind, SaveInput, Scope};

/// Who is saving. The system path (distill, promotion) may write a handoff and skips the quality
/// rules (its outputs are built to pass them); an agent may not and does not.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Agent,
    System,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::System => "system",
        }
    }
}

impl std::str::FromStr for Origin {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, String> {
        match value {
            "agent" => Ok(Self::Agent),
            "system" => Ok(Self::System),
            other => Err(format!("unknown origin `{other}`")),
        }
    }
}

/// A validated, redacted and derived memory, ready for the store to insert.
#[derive(Clone, Debug, PartialEq)]
pub struct NewMemory {
    pub kind: Kind,
    pub scope: Scope,
    pub origin: Origin,
    pub title: String,
    pub summary: String,
    pub reason: String,
    pub fields: Fields,
    pub evidence: Vec<String>,
    pub evidence_hash: String,
    pub identity_hash: String,
    pub context: Context,
    pub entities: Vec<Entity>,
    pub expires_at: Option<i64>,
    pub supersedes: Option<String>,
    pub contradicts: Option<String>,
}

/// Everything in §2.2 that needs no IO: redact, validate, apply the kind policy, hash, derive.
/// `has_project` is true when the person is a member of the request's project.
pub fn prepare_save(
    input: SaveInput,
    work: &WorkContext,
    has_project: bool,
    origin: Origin,
    now: i64,
) -> Result<NewMemory, Error> {
    let redact = |text: &str| redact_sensitive_content(text.trim());
    let fields =
        Fields::parse(input.kind, input.fields.clone())?.map_text(redact_sensitive_content);
    let input = SaveInput {
        title: redact(&input.title),
        summary: redact(&input.summary),
        reason: redact(&input.reason),
        evidence: input.evidence.iter().map(|e| redact(e)).collect(),
        ..input
    };
    validate_save(&input, &fields, origin == Origin::System)?;
    if origin == Origin::Agent {
        validate_quality(&input, &fields)?;
    }
    if input.supersedes.is_some() && input.supersedes == input.contradicts {
        return Err(Error::validation(
            "supersedes and contradicts must name different memories",
        ));
    }
    let scope = resolve_scope(input.kind, input.scope, has_project)?;
    let evidence = normalize_evidence(&input.evidence);
    let entities = entities::derive(
        &[
            input.title.as_str(),
            input.summary.as_str(),
            input.reason.as_str(),
            fields.text().as_str(),
        ],
        work,
    );
    Ok(NewMemory {
        kind: input.kind,
        scope,
        origin,
        identity_hash: identity_hash(input.kind, &input.title, &input.summary, &fields.to_json()),
        evidence_hash: evidence_hash(&evidence),
        title: input.title,
        summary: input.summary,
        reason: input.reason,
        fields,
        evidence,
        context: Context {
            repo: work.repository.clone(),
            branch: work.branch.clone(),
            worktree: work.worktree.clone(),
            commit: work.commit.clone(),
        },
        entities,
        expires_at: expires_at(input.kind, origin == Origin::System, scope, now),
        supersedes: input.supersedes,
        contradicts: input.contradicts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input(value: serde_json::Value) -> SaveInput {
        serde_json::from_value(value).expect("shape")
    }

    fn ok(value: serde_json::Value, has_project: bool) -> NewMemory {
        prepare_save(
            input(value),
            &WorkContext::default(),
            has_project,
            Origin::Agent,
            1_000,
        )
        .expect("valid")
    }

    #[test]
    fn rule_defaults_to_project_and_redacts_before_hashing() {
        let a = ok(
            json!({"kind":"rule","title":"No secrets in logs","summary":"token=abc123 never logged by any service","reason":"audit finding"}),
            true,
        );
        assert_eq!(a.scope, Scope::Project);
        assert!(!a.summary.contains("abc123"));
        let b = ok(
            json!({"kind":"rule","title":"No secrets in logs","summary":"token=zzz999 never logged by any service","reason":"audit finding"}),
            true,
        );
        assert_eq!(
            a.identity_hash, b.identity_hash,
            "identity is taken after redaction"
        );
        let personal = ok(
            json!({"kind":"rule","title":"No secrets in logs","summary":"Secrets never reach the log output.","reason":"audit finding"}),
            false,
        );
        assert_eq!(personal.scope, Scope::Personal);
    }

    #[test]
    fn note_expires_and_needs_no_reason_but_a_decision_does() {
        let note = ok(
            json!({"kind":"note","title":"A short-lived note","summary":"Something worth remembering for a while."}),
            true,
        );
        assert_eq!((note.scope, note.expires_at), (Scope::Personal, None));
        let err = prepare_save(
            input(
                json!({"kind":"decision","title":"Use one queue","summary":"Jobs run through one Postgres table."}),
            ),
            &WorkContext::default(),
            false,
            Origin::Agent,
            0,
        );
        assert!(matches!(err, Err(Error::Validation(m)) if m.contains("reason")));
    }

    #[test]
    fn wrong_or_unknown_fields_are_rejected() {
        let t = "A long enough title";
        let s = "A summary that says what is true.";
        for (value, needle) in [
            (
                json!({"kind":"fact","title":t,"summary":s,"reason":"because it is","fields":{"symptom":"x"}}),
                "symptom",
            ),
            (
                json!({"kind":"fact","title":t,"summary":s,"reason":"because it is","fields":"x"}),
                "object",
            ),
            (
                json!({"kind":"handoff","title":t,"summary":s,"reason":"because it is"}),
                "system",
            ),
            (
                json!({"kind":"preference","title":t,"summary":s,"reason":"because it is","scope":"project"}),
                "personal",
            ),
            (
                json!({"kind":"fact","title":t,"summary":s,"reason":"because it is","scope":"project"}),
                "initialized",
            ),
            (
                json!({"kind":"fact","title":"x".repeat(81),"summary":s,"reason":"because it is"}),
                "title",
            ),
            (
                json!({"kind":"fact","title":t,"summary":s,"reason":"because it is","evidence":vec!["e"; 21]}),
                "evidence",
            ),
            (
                json!({"kind":"fact","title":t,"summary":s,"reason":"because it is","supersedes":"a","contradicts":"a"}),
                "different",
            ),
            (
                json!({"kind":"procedure","title":t,"summary":s,"reason":"because it is","fields":{"steps":vec!["a"; 21]}}),
                "steps",
            ),
            (
                json!({"kind":"fact","title":t,"summary":s,"reason":"because it is","fields":{"source":"x".repeat(1001)}}),
                "source",
            ),
        ] {
            let result = serde_json::from_value::<SaveInput>(value)
                .map_err(|e| Error::validation(e.to_string()))
                .and_then(|i| prepare_save(i, &WorkContext::default(), false, Origin::Agent, 0));
            match result {
                Err(Error::Validation(message)) => {
                    assert!(message.contains(needle), "{message} lacks {needle}")
                }
                other => panic!("expected {needle} rejection, got {other:?}"),
            }
        }
        assert!(
            serde_json::from_value::<SaveInput>(
                json!({"kind":"fact","title":"t","summary":"s","bogus":1})
            )
            .is_err()
        );
    }

    #[test]
    fn agent_saves_are_quality_checked_and_system_saves_are_not() {
        let thin = json!({"kind":"gotcha","title":"t","summary":"s","reason":"r"});
        let agent = prepare_save(
            input(thin.clone()),
            &WorkContext::default(),
            false,
            Origin::Agent,
            0,
        );
        assert!(matches!(agent, Err(Error::Quality(m)) if m.starts_with("title:")));
        let system = prepare_save(
            input(thin),
            &WorkContext::default(),
            false,
            Origin::System,
            0,
        )
        .expect("system");
        assert_eq!(system.origin, Origin::System);
    }

    #[test]
    fn system_origin_may_write_a_handoff() {
        let handoff = prepare_save(
            input(json!({"kind":"handoff","title":"t","summary":"s","reason":"r","fields":{"next_steps":["a"]}})),
            &WorkContext::default(), false, Origin::System, 0,
        ).expect("system handoff");
        assert_eq!(handoff.expires_at, None);
    }
}
