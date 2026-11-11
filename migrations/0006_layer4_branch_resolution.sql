-- Layer 4 branch merge/close resolution metadata.

ALTER TABLE cairn_project_branches
    ADD COLUMN IF NOT EXISTS status_reason text,
    ADD COLUMN IF NOT EXISTS merge_commit text,
    ADD COLUMN IF NOT EXISTS merge_memory_id text;

CREATE INDEX IF NOT EXISTS cairn_project_branches_merge_memory_idx
    ON cairn_project_branches (person_id, merge_memory_id)
    WHERE merge_memory_id IS NOT NULL;

INSERT INTO cairn_schema_migrations (version)
VALUES ('0006_layer4_branch_resolution')
ON CONFLICT (version) DO NOTHING;
