ALTER TABLE cairn_memories
    ADD COLUMN IF NOT EXISTS contradicts jsonb NOT NULL DEFAULT '[]'::jsonb;

CREATE TABLE IF NOT EXISTS cairn_similarity_reviews (
    review_id text PRIMARY KEY,
    left_id text NOT NULL,
    right_id text NOT NULL,
    scope text NOT NULL,
    project_id text,
    cosine real NOT NULL,
    band text NOT NULL,
    status text NOT NULL,
    created_at bigint NOT NULL,
    resolved_at bigint,
    resolved_by text,
    UNIQUE (left_id, right_id)
);

CREATE INDEX IF NOT EXISTS cairn_similarity_reviews_open
    ON cairn_similarity_reviews (status, left_id, right_id);

INSERT INTO cairn_schema_migrations (version)
VALUES ('0017_contradictions')
ON CONFLICT (version) DO NOTHING;
