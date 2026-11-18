CREATE EXTENSION IF NOT EXISTS vector;
CREATE TABLE IF NOT EXISTS cairn_memory_embeddings (
    memory_id text PRIMARY KEY REFERENCES cairn_memories(memory_id) ON DELETE CASCADE,
    model_id text NOT NULL,
    model_version text NOT NULL,
    dimensions integer NOT NULL CHECK (dimensions = 384),
    normalized boolean NOT NULL CHECK (normalized),
    fingerprint text NOT NULL,
    source_revision text NOT NULL,
    created_at bigint NOT NULL,
    embedding vector(384) NOT NULL
);

INSERT INTO cairn_schema_migrations (version)
VALUES ('0014_hybrid_embeddings')
ON CONFLICT (version) DO NOTHING;
