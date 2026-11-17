use crate::{Memory, MemoryStore, StoreError, now_seconds};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const EMBEDDING_MODEL: &str = "Qdrant/all-MiniLM-L6-v2-onnx";
pub const EMBEDDING_VERSION: &str = "cb27c1f221563ca44e58ad2a694965db39c8153d:mean:256:v1";
pub const EMBEDDING_DIMENSIONS: usize = 384;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MemoryEmbedding {
    pub memory_id: String,
    pub model_id: String,
    pub model_version: String,
    pub dimensions: usize,
    pub normalized: bool,
    pub fingerprint: String,
    pub created_at: u64,
    pub vector: Vec<f32>,
}

impl MemoryEmbedding {
    pub fn new(memory: &Memory, vector: Vec<f32>) -> Result<Self, StoreError> {
        validate_vector(&vector)?;
        Ok(Self {
            memory_id: memory.memory_id.clone(),
            model_id: EMBEDDING_MODEL.to_owned(),
            model_version: EMBEDDING_VERSION.to_owned(),
            dimensions: EMBEDDING_DIMENSIONS,
            normalized: true,
            fingerprint: embedding_fingerprint(memory),
            created_at: now_seconds(),
            vector,
        })
    }

    pub fn matches(&self, memory: &Memory) -> bool {
        self.memory_id == memory.memory_id
            && self.model_id == EMBEDDING_MODEL
            && self.model_version == EMBEDDING_VERSION
            && self.dimensions == EMBEDDING_DIMENSIONS
            && self.normalized
            && self.fingerprint == embedding_fingerprint(memory)
    }
}

pub fn validate_vector(vector: &[f32]) -> Result<(), StoreError> {
    let norm = vector.iter().map(|v| v * v).sum::<f32>();
    if vector.len() != EMBEDDING_DIMENSIONS
        || vector.iter().any(|v| !v.is_finite())
        || !norm.is_finite()
        || (norm - 1.0).abs() > 0.01
    {
        return Err(StoreError::Validation(
            "embedding must be a finite normalized 384-dimensional vector".to_owned(),
        ));
    }
    Ok(())
}

pub fn embedding_text(m: &Memory) -> String {
    // Keep the title first and repeated; the tokenizer bounds the complete input to 256 tokens.
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
        m.fields.title,
        m.fields.title,
        m.fields.title,
        m.fields.summary,
        m.fields.details.as_deref().unwrap_or_default(),
        m.fields.reason,
        m.content,
        m.fields.tags.join(" "),
        m.fields.entities.join(" "),
        m.fields.applies_to.join(" "),
        m.context.files.join(" "),
        m.context.symbols.join(" "),
        m.context.commands.join(" ")
    )
    .chars()
    .take(8192)
    .collect()
}

pub fn embedding_fingerprint(memory: &Memory) -> String {
    format!("{:x}", Sha256::digest(embedding_text(memory).as_bytes()))
}

pub(crate) fn embedding_source_changed(before: &Memory, after: &Memory) -> bool {
    before.content != after.content
        || before.fields.title != after.fields.title
        || before.fields.summary != after.fields.summary
        || before.fields.details != after.fields.details
        || before.fields.reason != after.fields.reason
        || before.fields.tags != after.fields.tags
        || before.fields.entities != after.fields.entities
        || before.fields.applies_to != after.fields.applies_to
        || before.context.files != after.context.files
        || before.context.symbols != after.context.symbols
        || before.context.commands != after.context.commands
}

impl MemoryStore {
    /// Internal server job input, not an authenticated read surface.
    pub fn embedding_jobs(&self, limit: usize) -> Vec<Memory> {
        let hidden = self.live_superseded_targets();
        self.state
            .memories
            .iter()
            .filter(|m| self.memory_is_recalled(m, &hidden))
            .filter(|m| !self.state.embeddings.iter().any(|e| e.matches(m)))
            .filter(|m| {
                !self
                    .state
                    .job_quarantine
                    .iter()
                    .any(|row| row.kind == "embed" && row.key == m.memory_id)
            })
            .take(limit)
            .cloned()
            .collect()
    }

    /// A job racing correction, forgetting or another edit cannot publish a stale vector.
    pub fn put_embedding(&mut self, embedding: MemoryEmbedding) -> Result<bool, StoreError> {
        validate_vector(&embedding.vector)?;
        let hidden = self.live_superseded_targets();
        if !self
            .state
            .memories
            .iter()
            .any(|m| self.memory_is_recalled(m, &hidden) && embedding.matches(m))
        {
            return Ok(false);
        }
        self.state
            .embeddings
            .retain(|e| e.memory_id != embedding.memory_id);
        self.state.embeddings.push(embedding);
        self.persist()?;
        Ok(true)
    }

    pub(crate) fn semantic_scores(
        &self,
        memories: &[Memory],
        query: Option<&[f32]>,
    ) -> Result<Vec<(String, f32)>, StoreError> {
        let Some(query) = query else {
            return Ok(Vec::new());
        };
        validate_vector(query)?;
        let mut scores = Vec::new();
        for m in memories {
            if let Some(e) = self.state.embeddings.iter().find(|e| e.matches(m)) {
                validate_vector(&e.vector)?;
                let score = query.iter().zip(&e.vector).map(|(a, b)| a * b).sum();
                scores.push((m.memory_id.clone(), score));
            }
        }
        Ok(scores)
    }
}

#[cfg(test)]
mod tests;
