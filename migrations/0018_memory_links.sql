ALTER TABLE cairn_memories
    ADD COLUMN IF NOT EXISTS links jsonb NOT NULL DEFAULT '[]'::jsonb;

INSERT INTO cairn_schema_migrations (version)
VALUES ('0018_memory_links')
ON CONFLICT (version) DO NOTHING;
