-- Layer 5 explicit share policy, candidate dismissals, and share reasons.
-- Manual is the default when a project has no policy row. Turning the policy
-- off does not revoke shares that already exist.

ALTER TABLE cairn_project_shares
    ADD COLUMN IF NOT EXISTS share_reason text;

CREATE TABLE IF NOT EXISTS cairn_project_share_policies (
    owner_person_id text NOT NULL,
    project_id text NOT NULL,
    policy text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (owner_person_id, project_id),
    FOREIGN KEY (owner_person_id, project_id)
        REFERENCES cairn_projects (person_id, project_id)
);

CREATE TABLE IF NOT EXISTS cairn_share_candidate_dismissals (
    owner_person_id text NOT NULL,
    project_id text NOT NULL,
    memory_id text NOT NULL,
    member_person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (owner_person_id, project_id, memory_id, member_person_id),
    FOREIGN KEY (owner_person_id, project_id)
        REFERENCES cairn_projects (person_id, project_id)
);

INSERT INTO cairn_schema_migrations (version)
VALUES ('0009_layer5_share_policy')
ON CONFLICT (version) DO NOTHING;
