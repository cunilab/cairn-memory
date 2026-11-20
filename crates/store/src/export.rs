use crate::{Result, Store};
use cairn_core::memory::{Link, Memory};
use serde::Serialize;
use std::collections::HashSet;

/// The visible memories and the links between them.
#[derive(Clone, Debug, Serialize)]
pub struct Export {
    pub memories: Vec<Memory>,
    pub links: Vec<Link>,
}

impl Store {
    pub async fn export(&self, person_id: &str) -> Result<Export> {
        let memories = self.visible_memories(person_id).await?;
        let mut seen = HashSet::new();
        let links = memories
            .iter()
            .flat_map(|m| m.links.iter())
            .filter(|l| seen.insert((l.source.clone(), l.target.clone(), l.relation.as_str())))
            .cloned()
            .collect();
        Ok(Export { memories, links })
    }
}
