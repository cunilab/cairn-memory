use crate::error::Error;
use crate::id::sha256_hex;
use crate::memory::{Entity, Fields, Memory};

pub const EMBEDDING_MODEL: &str = "Qdrant/all-MiniLM-L6-v2-onnx";
pub const EMBEDDING_VERSION: &str = "cb27c1f221563ca44e58ad2a694965db39c8153d:mean:256:v1";
pub const EMBEDDING_DIMENSIONS: usize = 384;
pub const EMBED_BATCH: usize = 8;
const TEXT_MAX_CHARS: usize = 8192;

/// `model` column value: model id plus revision.
pub fn model_label() -> String {
    format!("{EMBEDDING_MODEL}@{EMBEDDING_VERSION}")
}

/// Title three times, then summary, reason, per-kind text and entity names.
pub fn embedding_text_parts(
    title: &str,
    summary: &str,
    reason: &str,
    fields: &Fields,
    entities: &[Entity],
) -> String {
    let names: Vec<&str> = entities.iter().map(|e| e.name.as_str()).collect();
    format!(
        "{title}\n{title}\n{title}\n{summary}\n{reason}\n{}\n{}",
        fields.text(),
        names.join(" ")
    )
    .chars()
    .take(TEXT_MAX_CHARS)
    .collect()
}

pub fn embedding_text(memory: &Memory) -> String {
    embedding_text_parts(
        &memory.title,
        &memory.summary,
        &memory.reason,
        &memory.fields,
        &memory.entities,
    )
}

/// Stored as `memory_embeddings.source_hash`; a changed text means the vector is stale.
pub fn source_hash(text: &str) -> String {
    sha256_hex(&[text])
}

pub fn validate_vector(vector: &[f32]) -> Result<(), Error> {
    let norm = vector.iter().map(|v| v * v).sum::<f32>();
    if vector.len() != EMBEDDING_DIMENSIONS
        || vector.iter().any(|v| !v.is_finite())
        || !norm.is_finite()
        || (norm - 1.0).abs() > 0.01
    {
        return Err(Error::validation(
            "embedding must be a finite normalized 384-dimensional vector",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Kind, fields::Fact};

    #[test]
    fn text_repeats_title_and_includes_fields_and_entities() {
        let fields = Fields::Fact(Fact {
            source: "the docs".into(),
        });
        let entities = [Entity {
            kind: crate::memory::EntityKind::File,
            name: "a/b.rs".into(),
        }];
        let text = embedding_text_parts("T", "S", "R", &fields, &entities);
        assert!(text.starts_with("T\nT\nT\nS\nR\n"));
        assert!(text.contains("the docs") && text.ends_with("a/b.rs"));
        assert_eq!(fields.kind(), Kind::Fact);
        assert_ne!(source_hash(&text), source_hash("other"));
    }

    #[test]
    fn vector_must_be_normalized_384() {
        let mut v = vec![0.0_f32; EMBEDDING_DIMENSIONS];
        v[0] = 1.0;
        assert!(validate_vector(&v).is_ok());
        assert!(validate_vector(&v[..10]).is_err());
        v[1] = f32::NAN;
        assert!(validate_vector(&v).is_err());
        assert!(validate_vector(&vec![0.0; EMBEDDING_DIMENSIONS]).is_err());
    }
}
