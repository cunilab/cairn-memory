use super::*;
use crate::test_support::temp_store_path;
use crate::{LifecycleInput, MemoryKind, WorkContext, build_explicit_memory_record};
use std::fs;

#[test]
fn vector_validation_and_provenance_reject_stale_or_malformed_jobs() {
    let mut memory = build_explicit_memory_record(
        "body",
        vec![],
        WorkContext::default(),
        LifecycleInput::default(),
        MemoryKind::Fact,
    )
    .unwrap();
    let mut vector = vec![0.0; EMBEDDING_DIMENSIONS];
    vector[0] = 1.0;
    let embedding = MemoryEmbedding::new(&memory, vector).unwrap();
    assert!(embedding.matches(&memory));
    for change in [
        |e: &mut MemoryEmbedding| e.model_id = "another model".into(),
        |e: &mut MemoryEmbedding| e.model_version = "another revision".into(),
        |e: &mut MemoryEmbedding| e.dimensions = 3,
        |e: &mut MemoryEmbedding| e.normalized = false,
    ] {
        let mut stale = embedding.clone();
        change(&mut stale);
        assert!(!stale.matches(&memory));
    }
    memory.fields.title = "changed".into();
    assert!(!embedding.matches(&memory));
    for vector in [vec![0.0; 384], vec![f32::NAN; 384], vec![1.0; 3]] {
        assert!(MemoryEmbedding::new(&memory, vector).is_err());
    }
}

#[test]
fn a_failed_vector_write_stays_pending_and_retries() -> Result<(), StoreError> {
    let path = temp_store_path("embedding-retry");
    let backup = path.with_extension("backup");
    let mut store = MemoryStore::open(&path)?;
    let memory = store.remember("A durable note", vec![], WorkContext::default())?;
    let mut vector = vec![0.0; EMBEDDING_DIMENSIONS];
    vector[0] = 1.0;
    let embedding = MemoryEmbedding::new(&memory, vector)?;
    fs::rename(&path, &backup)?;
    fs::create_dir(&path)?;
    assert!(store.put_embedding(embedding.clone()).is_err());
    assert!(store.state.embeddings.is_empty());
    assert_eq!(store.embedding_jobs(8), vec![memory]);
    fs::remove_dir(&path)?;
    fs::rename(&backup, &path)?;
    assert!(store.put_embedding(embedding)?);
    drop(store);
    assert!(MemoryStore::open(&path)?.embedding_jobs(8).is_empty());
    fs::remove_file(path)?;
    Ok(())
}
