ALTER TABLE cairn_memories
    ADD COLUMN IF NOT EXISTS access_count bigint NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS last_accessed_at timestamptz,
    ADD COLUMN IF NOT EXISTS helpful_count integer NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS stale_count integer NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS feedback jsonb NOT NULL DEFAULT '[]'::jsonb;

INSERT INTO cairn_schema_migrations (version)
VALUES ('0015_retention_feedback')
ON CONFLICT (version) DO NOTHING;
