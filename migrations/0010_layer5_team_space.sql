-- Layer 5 project team space. One team belongs to one project.
-- Removing a team member drops that project membership and revokes active
-- shares. The memory row stays stored.

CREATE TABLE IF NOT EXISTS cairn_project_teams (
    owner_person_id text NOT NULL,
    project_id text NOT NULL,
    name text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (owner_person_id, project_id),
    UNIQUE (project_id),
    FOREIGN KEY (owner_person_id, project_id)
        REFERENCES cairn_projects (person_id, project_id)
);

CREATE TABLE IF NOT EXISTS cairn_project_team_members (
    owner_person_id text NOT NULL,
    project_id text NOT NULL,
    member_person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (owner_person_id, project_id, member_person_id),
    FOREIGN KEY (owner_person_id, project_id)
        REFERENCES cairn_project_teams (owner_person_id, project_id)
);

INSERT INTO cairn_schema_migrations (version)
VALUES ('0010_layer5_team_space')
ON CONFLICT (version) DO NOTHING;
