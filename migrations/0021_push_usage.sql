-- Keep per-person usage when the short-lived session ledger is swept.
ALTER TABLE session_shown
    ADD COLUMN used_at timestamptz,
    ADD COLUMN inferred_use boolean NOT NULL DEFAULT false,
    ADD COLUMN error_repeated boolean NOT NULL DEFAULT false;

CREATE TABLE push_usage (
    person_id text NOT NULL REFERENCES accounts ON DELETE CASCADE,
    memory_id text NOT NULL REFERENCES memories ON DELETE CASCADE,
    pushed integer NOT NULL DEFAULT 0,
    used integer NOT NULL DEFAULT 0,
    muted boolean NOT NULL DEFAULT false,
    revived boolean NOT NULL DEFAULT false,
    PRIMARY KEY (person_id, memory_id)
);
CREATE INDEX push_usage_memory_idx ON push_usage (memory_id);

-- No backfill: the ledger shows pushes but not earlier gets or votes, so old rows would mute memories
-- on missing evidence. Every memory starts with a fresh observation window on its next push.
