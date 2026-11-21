-- Retrieval (CM-SPEC-007 §2): the embedding moves onto `memories` so the vector channel can filter
-- by visibility inside the HNSW scan (pgvector iterative index scan, >= 0.8.0).
DO $$ BEGIN
  IF (SELECT string_to_array(extversion, '.')::int[] FROM pg_extension WHERE extname = 'vector') < '{0,8,0}'
  THEN RAISE EXCEPTION 'pgvector >= 0.8.0 required (iterative index scans)'; END IF;
END $$;

ALTER TABLE memories
    ADD COLUMN embedding        vector(384),
    ADD COLUMN embedding_model  text,
    ADD COLUMN embedding_source text;            -- sha256 of core::embedding_text
DROP TABLE memory_embeddings;                    -- data is wipeable; embed jobs refill
CREATE INDEX memories_embedding_hnsw ON memories USING hnsw (embedding vector_cosine_ops)
    WHERE embedding IS NOT NULL AND status IN ('active','stale');
