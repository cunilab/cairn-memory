-- Console (CM-SPEC-009, CM-SPEC-010): what the web reports. Device client identity, which device
-- wrote what, retrieval usage, when a vote happened, which expiries were noted, why a memory is
-- stale, and a few server facts.
ALTER TABLE devices
    ADD COLUMN client_version text,
    ADD COLUMN agent          text CHECK (agent IN ('claude','codex','opencode')),
    ADD COLUMN last_hook_at   timestamptz,
    ADD COLUMN legacy_hook    boolean NOT NULL DEFAULT false;     -- a Python-urllib client

ALTER TABLE sessions ADD COLUMN device_id text;                    -- the device that captured it
ALTER TABLE memories
    ADD COLUMN device_id         text,                             -- the device that saved it
    ADD COLUMN missing_entities  text[] NOT NULL DEFAULT '{}',     -- cited files and symbols found gone
    ADD COLUMN expiry_noted_for  timestamptz;                      -- the expires_at a sweep recorded
UPDATE memories SET expiry_noted_for = expires_at WHERE expires_at <= now();

ALTER TABLE usage_votes ADD COLUMN created_at timestamptz NOT NULL DEFAULT now();

-- One row per person, 15-minute bucket and kind of lookup, so the web can group by any local day.
CREATE TABLE retrieval_log (
    person_id text NOT NULL REFERENCES accounts ON DELETE CASCADE,
    bucket    timestamptz NOT NULL,
    kind      text NOT NULL CHECK (kind IN ('search','get','brief','topic')),
    calls     integer NOT NULL DEFAULT 0,
    bytes     bigint NOT NULL DEFAULT 0,                           -- response size; tokens = bytes / 4
    hits      integer NOT NULL DEFAULT 0,                          -- lookups that found something
    PRIMARY KEY (person_id, bucket, kind)
);

CREATE TABLE server_state (key text PRIMARY KEY, value text NOT NULL);
