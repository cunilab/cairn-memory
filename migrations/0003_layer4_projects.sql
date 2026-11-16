-- Layer 4 project registry and explicit promotion provenance.

CREATE TABLE IF NOT EXISTS cairn_projects (
    person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    project_id text NOT NULL,
    repository text NOT NULL,
    name text NOT NULL,
    default_branch text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id),
    UNIQUE (person_id, project_id)
);

ALTER TABLE cairn_memories
    ADD COLUMN IF NOT EXISTS project_id text,
    ADD COLUMN IF NOT EXISTS promoted_from text,
    ADD COLUMN IF NOT EXISTS promotion_reason text,
    ADD COLUMN IF NOT EXISTS published_by text REFERENCES cairn_accounts(person_id),
    ADD COLUMN IF NOT EXISTS changed_by text REFERENCES cairn_accounts(person_id),
    ADD COLUMN IF NOT EXISTS changed_at text;

CREATE INDEX IF NOT EXISTS cairn_memories_project_idx
    ON cairn_memories (person_id, project_id, created_at DESC);

INSERT INTO cairn_schema_migrations (version)
VALUES ('0003_layer4_projects')
ON CONFLICT (version) DO NOTHING;
