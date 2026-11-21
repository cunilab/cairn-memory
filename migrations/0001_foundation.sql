-- Cairn 0.1.6 foundation schema. Unqualified names: the runner sets
-- search_path = <schema>, public (production: cairn; tests: t_<random>).
-- The `vector` extension lives in public.

CREATE TABLE schema_version (version text PRIMARY KEY, applied_at timestamptz NOT NULL DEFAULT now());

-- account ------------------------------------------------------------------
CREATE TABLE accounts (
    person_id     text PRIMARY KEY,
    email         text NOT NULL UNIQUE,            -- normalized lower-case
    password_hash text NOT NULL,                   -- argon2
    is_admin      boolean NOT NULL DEFAULT false,
    status        text NOT NULL DEFAULT 'active' CHECK (status IN ('active','disabled')),
    created_at    timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX accounts_one_admin ON accounts ((true)) WHERE is_admin;

CREATE TABLE web_sessions (                        -- web console bearer (cookie)
    session_id text PRIMARY KEY,
    person_id  text NOT NULL REFERENCES accounts,
    token_hash text NOT NULL UNIQUE,               -- sha256
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz
);
CREATE INDEX web_sessions_person_idx ON web_sessions (person_id, expires_at);

CREATE TABLE devices (                             -- device token = agent credential
    device_id    text PRIMARY KEY,
    person_id    text NOT NULL REFERENCES accounts,
    name         text NOT NULL,
    token_hash   text NOT NULL UNIQUE,
    status       text NOT NULL DEFAULT 'active' CHECK (status IN ('active','revoked')),
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX devices_person_idx ON devices (person_id, status);

CREATE TABLE audit_events (                        -- metadata only; feeds the web Activity page
    audit_id   text PRIMARY KEY,
    person_id  text NOT NULL REFERENCES accounts,
    device_id  text,
    action     text NOT NULL,
    subject_id text,
    metadata   jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX audit_events_person_idx ON audit_events (person_id, created_at DESC);

-- project ------------------------------------------------------------------
CREATE TABLE projects (
    project_id     text PRIMARY KEY,               -- from committed .cairn/project.json
    owner_id       text NOT NULL REFERENCES accounts,
    repository     text NOT NULL,                  -- host/org/repo
    name           text NOT NULL,
    default_branch text,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE project_members (                     -- owner is also a member row
    project_id text NOT NULL REFERENCES projects ON DELETE CASCADE,
    person_id  text NOT NULL REFERENCES accounts,
    joined_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, person_id)
);
CREATE INDEX project_members_person_idx ON project_members (person_id);

-- memory -------------------------------------------------------------------
CREATE TABLE memories (
    memory_id      text PRIMARY KEY,               -- core::new_id("mem")
    owner_id       text NOT NULL REFERENCES accounts,
    project_id     text REFERENCES projects,
    scope          text NOT NULL CHECK (scope IN ('personal','project','global')),
    kind           text NOT NULL CHECK (kind IN
                     ('decision','rule','gotcha','procedure','fact','preference','note','handoff')),
    title          text NOT NULL CHECK (char_length(title) <= 80),
    summary        text NOT NULL CHECK (char_length(summary) <= 240),
    reason         text NOT NULL DEFAULT '',
    fields         jsonb NOT NULL DEFAULT '{}',    -- per-kind object, validated in core
    evidence       text[] NOT NULL DEFAULT '{}',
    evidence_count integer NOT NULL DEFAULT 0,     -- includes evidence merged by consolidate
    evidence_hash  text NOT NULL,                  -- sha256 of sorted evidence
    identity_hash  text NOT NULL,                  -- sha256 of kind,title,summary,reason,fields
    repo           text,                           -- context, from the capturing client
    repo_branch    text,
    repo_worktree  text,
    repo_commit    text,
    status         text NOT NULL DEFAULT 'active'
                     CHECK (status IN ('active','stale','superseded','forgotten')),
    expires_at     timestamptz,                    -- set from kind (§2.3)
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    search_tsv     tsvector GENERATED ALWAYS AS (
        setweight(to_tsvector('english', title), 'A') ||
        setweight(to_tsvector('english', summary), 'B') ||
        setweight(to_tsvector('english', reason), 'C') ||
        setweight(jsonb_to_tsvector('english', fields, '["string"]'), 'C')
    ) STORED,
    CHECK (scope <> 'project' OR project_id IS NOT NULL)
);
-- visibility pre-filters (step 5 joins search candidates against these)
CREATE INDEX memories_owner_idx   ON memories (owner_id, status, updated_at DESC);
CREATE INDEX memories_project_idx ON memories (project_id, status, updated_at DESC)
    WHERE scope = 'project';
CREATE INDEX memories_global_idx  ON memories (updated_at DESC)
    WHERE scope = 'global' AND status = 'active';
CREATE INDEX memories_branch_idx  ON memories (project_id, repo_branch) WHERE repo_branch IS NOT NULL;
CREATE INDEX memories_search_idx  ON memories USING gin (search_tsv);        -- websearch_to_tsquery
CREATE INDEX memories_expiry_idx  ON memories (expires_at) WHERE expires_at IS NOT NULL;
CREATE UNIQUE INDEX memories_identity_idx                                    -- exact dedupe
    ON memories (owner_id, scope, coalesce(project_id, ''), identity_hash);

CREATE TABLE memory_embeddings (
    memory_id   text PRIMARY KEY REFERENCES memories ON DELETE CASCADE,
    model       text NOT NULL,                     -- e.g. 'all-MiniLM-L6-v2@<rev>'
    source_hash text NOT NULL,                     -- sha256 of core::embedding_text
    embedding   vector(384) NOT NULL,              -- L2-normalized
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX memory_embeddings_hnsw ON memory_embeddings
    USING hnsw (embedding vector_cosine_ops);                                -- step-5 vector top-K

CREATE TABLE entities (                            -- server-derived, deterministic
    memory_id text NOT NULL REFERENCES memories ON DELETE CASCADE,
    kind      text NOT NULL CHECK (kind IN ('file','symbol','command','test','service','term')),
    name      text NOT NULL,                       -- normalized (repo-relative path, lower-case term)
    PRIMARY KEY (memory_id, kind, name)
);
CREATE INDEX entities_lookup_idx ON entities (name, kind);                    -- entity channel + auto-links

CREATE TABLE links (
    source_id  text NOT NULL REFERENCES memories ON DELETE CASCADE,
    target_id  text NOT NULL REFERENCES memories ON DELETE CASCADE,
    relation   text NOT NULL CHECK (relation IN
                 ('relates','causes','fixes','depends_on','supersedes','contradicts')),
    origin     text NOT NULL CHECK (origin IN ('agent','auto')),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (source_id, target_id, relation),
    CHECK (source_id <> target_id)
);
CREATE INDEX links_target_idx ON links (target_id, relation);

CREATE TABLE usage (                               -- internal; never shown to agents
    memory_id    text PRIMARY KEY REFERENCES memories ON DELETE CASCADE,
    fetched      integer NOT NULL DEFAULT 0,       -- explicit get/search by an agent only
    helpful      integer NOT NULL DEFAULT 0,
    wrong        integer NOT NULL DEFAULT 0,
    last_used_at timestamptz
);

CREATE TABLE usage_votes (                         -- one row per voter and vote; feeds usage.helpful/wrong
    memory_id text NOT NULL REFERENCES memories ON DELETE CASCADE,
    person_id text NOT NULL REFERENCES accounts,
    vote      text NOT NULL CHECK (vote IN ('helpful','wrong')),
    PRIMARY KEY (memory_id, person_id, vote)
);

-- capture ------------------------------------------------------------------
CREATE TABLE sessions (
    person_id     text NOT NULL REFERENCES accounts,
    session_id    text NOT NULL,                   -- host agent's session id
    project_id    text REFERENCES projects,
    context       jsonb NOT NULL DEFAULT '{}',     -- repo, branch, worktree, commit
    started_at    timestamptz NOT NULL DEFAULT now(),
    ended_at      timestamptz,
    pending_since timestamptz,                     -- has captures not yet distilled
    PRIMARY KEY (person_id, session_id)
);
CREATE INDEX sessions_pending_idx ON sessions (pending_since) WHERE pending_since IS NOT NULL;

CREATE TABLE observations (                        -- raw evidence; never recalled
    person_id   text NOT NULL REFERENCES accounts,
    event_id    text NOT NULL,                     -- client idempotency key
    session_id  text NOT NULL,
    role        text NOT NULL,
    source      text NOT NULL,
    content     text NOT NULL,                     -- redacted, clipped to 2 KiB
    content_hash text NOT NULL,                    -- same event_id with other content = 409
    context     jsonb NOT NULL DEFAULT '{}',       -- WorkContext incl. files/symbols/commands
    captured_at timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL,              -- captured_at + 30 days
    PRIMARY KEY (person_id, event_id),
    FOREIGN KEY (person_id, session_id) REFERENCES sessions
);
CREATE INDEX observations_session_idx ON observations (person_id, session_id, captured_at);
CREATE INDEX observations_expiry_idx  ON observations (expires_at);

-- process ------------------------------------------------------------------
CREATE TABLE jobs (
    job_id           text PRIMARY KEY,
    kind             text NOT NULL CHECK (kind IN ('embed','distill','consolidate')),
    job_key          text NOT NULL,
    status           text NOT NULL DEFAULT 'pending'
                       CHECK (status IN ('pending','running','done','quarantined')),
    attempts         integer NOT NULL DEFAULT 0,
    not_before       timestamptz NOT NULL DEFAULT now(),
    claim_id         text,
    lease_expires_at timestamptz,
    last_error       text,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    finished_at      timestamptz
);
CREATE UNIQUE INDEX jobs_one_pending ON jobs (kind, job_key) WHERE status = 'pending';
CREATE INDEX jobs_claimable ON jobs (kind, not_before, job_id) WHERE status = 'pending';
CREATE INDEX jobs_lease     ON jobs (lease_expires_at) WHERE status = 'running';
CREATE INDEX jobs_admin     ON jobs (status, updated_at DESC) WHERE status <> 'done';
