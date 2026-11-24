-- Superseded memories stay searchable. The 0003 partial index omits them, so this wider one
-- covers the search predicate. The old index is kept.
CREATE INDEX memories_embedding_hnsw_always ON memories USING hnsw (embedding vector_cosine_ops)
    WHERE embedding IS NOT NULL AND status IN ('active', 'stale', 'superseded');
