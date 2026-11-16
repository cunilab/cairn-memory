-- Layer 5 project team space. One team belongs to one project; its members are the
-- project's members (cairn_project_members), who join through a capture, so there is no
-- separate member table.

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

INSERT INTO cairn_schema_migrations (version)
VALUES ('0010_layer5_team_space')
ON CONFLICT (version) DO NOTHING;
