-- Layer 4 first-class project evidence entities and verification history.

CREATE TABLE IF NOT EXISTS cairn_project_entities (
    person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    entity_id text NOT NULL,
    project_id text NOT NULL,
    kind text NOT NULL CHECK (kind IN ('file', 'symbol', 'decision', 'handoff', 'runbook')),
    entity_key text NOT NULL,
    content text,
    source_memory_id text,
    commit_sha text,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'retired')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (person_id, entity_id),
    UNIQUE (person_id, project_id, kind, entity_key),
    FOREIGN KEY (person_id, project_id)
        REFERENCES cairn_projects (person_id, project_id)
);

CREATE TABLE IF NOT EXISTS cairn_project_freshness_events (
    person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    event_id text NOT NULL,
    project_id text NOT NULL,
    memory_id text NOT NULL,
    previous_commit text,
    commit_sha text NOT NULL,
    action text NOT NULL CHECK (action IN ('promotion', 'merge', 'verification')),
    reason text,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (person_id, event_id),
    FOREIGN KEY (person_id, project_id)
        REFERENCES cairn_projects (person_id, project_id)
);

CREATE INDEX IF NOT EXISTS cairn_project_entities_project_idx
    ON cairn_project_entities (person_id, project_id, kind, updated_at DESC);

CREATE INDEX IF NOT EXISTS cairn_project_freshness_project_idx
    ON cairn_project_freshness_events (person_id, project_id, created_at DESC);

INSERT INTO cairn_schema_migrations (version)
VALUES ('0007_layer4_registry_maintenance')
ON CONFLICT (version) DO NOTHING;
