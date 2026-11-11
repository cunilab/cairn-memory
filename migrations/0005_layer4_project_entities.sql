-- Layer 4 project repository branches and worktrees.

CREATE TABLE IF NOT EXISTS cairn_project_branches (
    person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    branch_id text NOT NULL,
    project_id text NOT NULL,
    name text NOT NULL,
    base_branch text,
    base_commit text,
    head_commit text,
    status text NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'merged', 'closed')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (person_id, branch_id),
    UNIQUE (person_id, project_id, name),
    FOREIGN KEY (person_id, project_id)
        REFERENCES cairn_projects (person_id, project_id)
);

CREATE TABLE IF NOT EXISTS cairn_project_worktrees (
    person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    worktree_id text NOT NULL,
    project_id text NOT NULL,
    branch_id text,
    path text NOT NULL,
    commit_sha text,
    status text NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'removed')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (person_id, worktree_id),
    UNIQUE (person_id, project_id, path),
    FOREIGN KEY (person_id, project_id)
        REFERENCES cairn_projects (person_id, project_id)
);

CREATE INDEX IF NOT EXISTS cairn_project_branches_project_idx
    ON cairn_project_branches (person_id, project_id, updated_at DESC);

CREATE INDEX IF NOT EXISTS cairn_project_worktrees_project_idx
    ON cairn_project_worktrees (person_id, project_id, updated_at DESC);

INSERT INTO cairn_schema_migrations (version)
VALUES ('0005_layer4_project_entities')
ON CONFLICT (version) DO NOTHING;
