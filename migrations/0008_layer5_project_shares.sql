-- Shared project membership: every member reads every live published decision.

CREATE TABLE IF NOT EXISTS cairn_project_members (
    owner_person_id text NOT NULL,
    project_id text NOT NULL,
    member_person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, member_person_id),
    FOREIGN KEY (project_id)
        REFERENCES cairn_projects (project_id)
);

INSERT INTO cairn_schema_migrations (version)
VALUES ('0008_layer5_project_shares')
ON CONFLICT (version) DO NOTHING;
