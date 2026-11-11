-- Cairn Layer 3 canonical server schema.
-- Application-generated identifiers keep this migration independent of a UUID
-- extension. Search indexes are derived and may be rebuilt from canonical rows.

CREATE TABLE IF NOT EXISTS cairn_schema_migrations (
    version text PRIMARY KEY,
    applied_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS cairn_accounts (
    person_id text PRIMARY KEY,
    email text NOT NULL UNIQUE,
    password_hash text NOT NULL,
    status text NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'disabled')),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS cairn_auth_sessions (
    session_id text PRIMARY KEY,
    person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    token_hash text NOT NULL UNIQUE,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz
);

CREATE INDEX IF NOT EXISTS cairn_auth_sessions_person_idx
    ON cairn_auth_sessions (person_id, expires_at);

CREATE TABLE IF NOT EXISTS cairn_devices (
    device_id text PRIMARY KEY,
    person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    agent_id text,
    name text NOT NULL,
    token_hash text NOT NULL UNIQUE,
    status text NOT NULL DEFAULT 'active'
        CHECK (status IN ('active', 'revoked')),
    registered_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    pull_cursor bigint NOT NULL DEFAULT 0 CHECK (pull_cursor >= 0),
    UNIQUE (device_id, person_id)
);

CREATE INDEX IF NOT EXISTS cairn_devices_person_idx
    ON cairn_devices (person_id, status);

CREATE TABLE IF NOT EXISTS cairn_sessions (
    session_id text NOT NULL,
    person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    context jsonb NOT NULL DEFAULT '{}'::jsonb,
    started_at timestamptz NOT NULL DEFAULT now(),
    ended_at timestamptz,
    PRIMARY KEY (person_id, session_id)
);

CREATE TABLE IF NOT EXISTS cairn_observations (
    person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    event_id text NOT NULL,
    session_id text NOT NULL,
    content text NOT NULL,
    role text NOT NULL,
    source text NOT NULL,
    importance text,
    lifecycle jsonb NOT NULL DEFAULT '{}'::jsonb,
    scope text NOT NULL DEFAULT 'session',
    context jsonb NOT NULL DEFAULT '{}'::jsonb,
    captured_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (person_id, event_id),
    FOREIGN KEY (person_id, session_id)
        REFERENCES cairn_sessions (person_id, session_id)
);

CREATE TABLE IF NOT EXISTS cairn_sync_events (
    person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    event_id text NOT NULL,
    cursor bigint GENERATED ALWAYS AS IDENTITY,
    device_id text NOT NULL,
    accepted_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (person_id, event_id),
    UNIQUE (person_id, cursor),
    FOREIGN KEY (person_id, event_id)
        REFERENCES cairn_observations (person_id, event_id),
    FOREIGN KEY (device_id, person_id)
        REFERENCES cairn_devices (device_id, person_id)
);

CREATE TABLE IF NOT EXISTS cairn_memories (
    memory_id text PRIMARY KEY,
    person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    session_id text NOT NULL,
    content text NOT NULL,
    source_event_ids jsonb NOT NULL DEFAULT '[]'::jsonb,
    context jsonb NOT NULL DEFAULT '{}'::jsonb,
    lifecycle jsonb NOT NULL DEFAULT '{}'::jsonb,
    kind text NOT NULL DEFAULT 'explicit',
    scope text NOT NULL DEFAULT 'global',
    next_steps jsonb NOT NULL DEFAULT '[]'::jsonb,
    unresolved_work jsonb NOT NULL DEFAULT '[]'::jsonb,
    supersedes text,
    created_at timestamptz NOT NULL DEFAULT now(),
    search_tsv tsvector GENERATED ALWAYS AS (to_tsvector('simple', content)) STORED
);

CREATE INDEX IF NOT EXISTS cairn_memories_person_scope_idx
    ON cairn_memories (person_id, scope, created_at DESC);

CREATE INDEX IF NOT EXISTS cairn_memories_search_idx
    ON cairn_memories USING gin (search_tsv);

CREATE TABLE IF NOT EXISTS cairn_forgotten_items (
    person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    item_id text NOT NULL,
    forgotten_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (person_id, item_id)
);

INSERT INTO cairn_schema_migrations (version)
VALUES ('0001_layer3_personal_memory')
ON CONFLICT (version) DO NOTHING;
