//! Pure builders for core unit tests. No IO.

use crate::memory::model::Rank;
use crate::memory::{Context, Fields, Kind, Memory, Scope, Status, Usage, fields};
use crate::process::Origin;

pub fn memory(id: &str, kind: Kind, scope: Scope) -> Memory {
    Memory {
        id: id.to_owned(),
        owner: "owner".to_owned(),
        project_id: (scope == Scope::Project).then(|| "p1".to_owned()),
        scope,
        kind,
        title: format!("title {id}"),
        summary: format!("summary {id}"),
        reason: String::new(),
        detail: String::new(),
        fields: Fields::Note(fields::Note {}),
        evidence: Vec::new(),
        evidence_count: 0,
        context: Context::default(),
        entities: Vec::new(),
        links: Vec::new(),
        status: Status::Active,
        rank: Rank::Normal,
        faded_at: None,
        edited_by: false,
        proof_count: 1,
        stale_cause: None,
        origin: Origin::Agent,
        expires_at: None,
        created_at: 0,
        created_us: 0,
        updated_at: 0,
        evidence_hash: String::new(),
        identity_hash: String::new(),
        usage: Usage::default(),
    }
}
