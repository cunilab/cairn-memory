use super::fields::Fields;
use crate::process::Origin;
use serde::{Deserialize, Serialize};

macro_rules! text_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }
        }

        impl std::str::FromStr for $name {
            type Err = String;
            fn from_str(value: &str) -> Result<Self, String> {
                match value {
                    $($text => Ok(Self::$variant),)+
                    other => Err(format!("unknown {} `{other}`", stringify!($name).to_lowercase())),
                }
            }
        }
    };
}

text_enum!(Kind {
    Decision => "decision", Rule => "rule", Gotcha => "gotcha", Procedure => "procedure",
    Fact => "fact", Preference => "preference", Note => "note", Handoff => "handoff",
});
text_enum!(Scope { Personal => "personal", Project => "project", Global => "global" });
text_enum!(Status {
    Active => "active", Stale => "stale", Superseded => "superseded", Forgotten => "forgotten",
});
text_enum!(Rank { Candidate => "candidate", Normal => "normal" });
text_enum!(TrustLevel { Doubted => "doubted", New => "new", Proven => "proven", Normal => "normal" });
text_enum!(Relation {
    Relates => "relates", Causes => "causes", Fixes => "fixes", DependsOn => "depends_on",
    Supersedes => "supersedes", Contradicts => "contradicts",
});
text_enum!(EntityKind {
    File => "file", Module => "module", Symbol => "symbol", Command => "command", Test => "test",
    Service => "service", Term => "term",
});

/// The client's git state. Never redacted.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub commit: Option<String>,
}

/// What an agent may send. Everything else is server-derived.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveInput {
    /// `handoff` is rejected: system-only.
    pub kind: Kind,
    /// 1..=80 chars.
    pub title: String,
    /// 1..=240 chars.
    pub summary: String,
    /// Required (non-empty) unless `kind` is `note`.
    #[serde(default)]
    pub reason: String,
    /// Optional long context, up to 8,000 characters.
    #[serde(default)]
    pub detail: Option<String>,
    /// Parsed by `Fields::parse(kind, ..)`.
    #[serde(default)]
    pub fields: Option<serde_json::Value>,
    /// At most 20 items of at most 300 chars.
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub supersedes: Option<String>,
    #[serde(default)]
    pub contradicts: Option<String>,
    #[serde(default)]
    pub scope: Option<Scope>,
    /// Up to 5 short questions, each 1..=120 characters. Not part of `identity_hash`.
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// A stored memory as the server returns it.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Memory {
    pub id: String,
    pub owner: String,
    pub project_id: Option<String>,
    pub scope: Scope,
    pub kind: Kind,
    pub title: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(skip)]
    pub detail: String,
    /// Serialized as the bare per-kind object.
    pub fields: Fields,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    pub evidence_count: u32,
    pub context: Context,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entities: Vec<Entity>,
    /// Outgoing and incoming. The console shows them through `MemoryView`, which adds the other end.
    #[serde(skip)]
    pub links: Vec<Link>,
    pub status: Status,
    pub rank: Rank,
    #[serde(skip)]
    pub faded_at: Option<i64>,
    /// Whether an administrator edited this memory; the editor's identity stays private.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub edited_by: bool,
    /// Times this knowledge was saved, including the first. At least 1.
    /// Omitted the first time, same as a search card's `seen`.
    #[serde(default, skip_serializing_if = "proof_once")]
    pub proof_count: u32,
    /// Why a stale memory went stale: `feedback` (wrong votes) or `missing` (cited code is gone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_cause: Option<String>,
    /// Who wrote it. Internal: agents see only the memory.
    #[serde(skip)]
    pub origin: Origin,
    /// Unix seconds.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub expires_at: Option<i64>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub created_at: i64,
    /// Microsecond creation time for tie-breaks; `created_at` has one-second resolution.
    #[serde(skip)]
    pub created_us: i64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub updated_at: i64,
    #[serde(skip)]
    pub evidence_hash: String,
    #[serde(skip)]
    pub identity_hash: String,
    /// Internal only; never shown to agents.
    #[serde(skip)]
    pub usage: Usage,
}

fn proof_once(n: &u32) -> bool {
    *n < 2
}

/// What an agent sees of a stored memory (`memory_get`), including its trust reason.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Record {
    pub id: String,
    pub kind: Kind,
    pub scope: Scope,
    pub status: Status,
    /// Why a stale record is stale: `feedback` (wrong votes) or `missing` (cited code is gone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_cause: Option<String>,
    pub title: String,
    pub summary: String,
    pub trust: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub detail_chars: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Fields::is_empty")]
    pub fields: Fields,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub updated_at: i64,
}

impl From<&Memory> for Record {
    fn from(m: &Memory) -> Self {
        Self {
            id: m.id.clone(),
            kind: m.kind,
            scope: m.scope,
            status: m.status,
            stale_cause: m.stale_cause.clone(),
            title: m.title.clone(),
            summary: m.summary.clone(),
            trust: m.trust().1,
            reason: m.reason.clone(),
            detail_chars: (!m.detail.is_empty()).then(|| m.detail.chars().count()),
            detail: None,
            fields: m.fields.clone(),
            evidence: m.evidence.clone(),
            links: m.links.clone(),
            branch: m.context.branch.clone(),
            updated_at: m.updated_at,
        }
    }
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Entity {
    pub kind: EntityKind,
    pub name: String,
}

#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Link {
    pub relation: Relation,
    pub source: String,
    pub target: String,
    pub auto: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub fetched: u32,
    pub shown: u32,
    pub detail_opened: u32,
    pub helpful: u32,
    pub wrong: u32,
    pub last_used_at: Option<i64>,
    pub last_shown_at: Option<i64>,
}
