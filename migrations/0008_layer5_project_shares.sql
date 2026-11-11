-- Layer 5 explicit project membership and one-memory shares.
-- A share grants read access to one promoted project memory. It does not copy
-- the memory, change device sync, or grant correct/forget rights.

CREATE TABLE IF NOT EXISTS cairn_project_members (
    owner_person_id text NOT NULL,
    project_id text NOT NULL,
    member_person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (owner_person_id, project_id, member_person_id),
    FOREIGN KEY (owner_person_id, project_id)
        REFERENCES cairn_projects (person_id, project_id)
);

CREATE TABLE IF NOT EXISTS cairn_project_shares (
    owner_person_id text NOT NULL,
    project_id text NOT NULL,
    memory_id text NOT NULL,
    member_person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    created_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz,
    PRIMARY KEY (owner_person_id, project_id, memory_id, member_person_id),
    FOREIGN KEY (owner_person_id, project_id)
        REFERENCES cairn_projects (person_id, project_id)
);

CREATE INDEX IF NOT EXISTS cairn_project_shares_member_idx
    ON cairn_project_shares (member_person_id, project_id)
    WHERE revoked_at IS NULL;

INSERT INTO cairn_schema_migrations (version)
VALUES ('0008_layer5_project_shares')
ON CONFLICT (version) DO NOTHING;
