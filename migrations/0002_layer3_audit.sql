-- Layer 3 operational audit records. Payloads are intentionally metadata-only;
-- memory content, bearer tokens, and passwords never enter this table.

CREATE TABLE IF NOT EXISTS cairn_audit_events (
    audit_id text PRIMARY KEY,
    person_id text NOT NULL REFERENCES cairn_accounts(person_id),
    actor_device_id text,
    action text NOT NULL,
    subject_id text,
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS cairn_audit_events_person_idx
    ON cairn_audit_events (person_id, created_at DESC);

INSERT INTO cairn_schema_migrations (version)
VALUES ('0002_layer3_audit')
ON CONFLICT (version) DO NOTHING;
