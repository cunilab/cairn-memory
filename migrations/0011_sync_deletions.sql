-- Sync deletions share the observation cursor sequence so a later pull can
-- order a tombstone after an event the device already accepted.
-- A device that never pulls again is not purged.

CREATE TABLE IF NOT EXISTS cairn_sync_deletions (
    person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    item_id text NOT NULL,
    cursor bigint NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (person_id, cursor)
);

CREATE INDEX IF NOT EXISTS cairn_sync_deletions_person_item_idx
    ON cairn_sync_deletions (person_id, item_id);

INSERT INTO cairn_schema_migrations (version)
VALUES ('0011_sync_deletions')
ON CONFLICT (version) DO NOTHING;
