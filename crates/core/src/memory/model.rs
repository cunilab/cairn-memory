use super::fields::Fields;
use serde::{Deserialize, Serialize};

macro_rules! text_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
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
text_enum!(Relation {
    Relates => "relates", Causes => "causes", Fixes => "fixes", DependsOn => "depends_on",
    Supersedes => "supersedes", Contradicts => "contradicts",
});
text_enum!(EntityKind {
    File => "file", Symbol => "symbol", Command => "command", Test => "test",
    Service => "service", Term => "term",
});

/// The client's git state. Never redacted.
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
}

/// A stored memory as the server returns it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Memory {
    pub id: String,
    pub owner: String,
    pub project_id: Option<String>,
    pub scope: Scope,
    pub kind: Kind,
    pub title: String,
    pub summary: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// Serialized as the bare per-kind object.
    pub fields: Fields,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    pub evidence_count: u32,
    pub context: Context,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub entities: Vec<Entity>,
    /// Outgoing and incoming.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
    pub status: Status,
    /// Unix seconds.
    pub expires_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip)]
    pub evidence_hash: String,
    #[serde(skip)]
    pub identity_hash: String,
    /// Internal only; never shown to agents.
    #[serde(skip)]
    pub usage: Usage,
}

/// What an agent sees of a stored memory (`memory_get`): no owner, project or usage.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Record {
    pub id: String,
    pub kind: Kind,
    pub scope: Scope,
    pub status: Status,
    pub title: String,
    pub summary: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(skip_serializing_if = "Fields::is_empty")]
    pub fields: Fields,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub updated_at: i64,
}

impl From<&Memory> for Record {
    fn from(m: &Memory) -> Self {
        Self {
            id: m.id.clone(),
            kind: m.kind,
            scope: m.scope,
            status: m.status,
            title: m.title.clone(),
            summary: m.summary.clone(),
            reason: m.reason.clone(),
            fields: m.fields.clone(),
            evidence: m.evidence.clone(),
            links: m.links.clone(),
            branch: m.context.branch.clone(),
            updated_at: m.updated_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Entity {
    pub kind: EntityKind,
    pub name: String,
}

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
    pub helpful: u32,
    pub wrong: u32,
    pub last_used_at: Option<i64>,
}
