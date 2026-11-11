-- Layer 4 verification commit for freshness-aware project retrieval.

ALTER TABLE cairn_memories
    ADD COLUMN IF NOT EXISTS verified_commit text;

INSERT INTO cairn_schema_migrations (version)
VALUES ('0004_layer4_freshness')
ON CONFLICT (version) DO NOTHING;
