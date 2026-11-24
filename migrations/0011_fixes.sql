CREATE TABLE fixes (
    fix_id text PRIMARY KEY, detector text NOT NULL, fingerprint text NOT NULL,
    subject_kind text, subject_id text, project_id text, device_id text, person_id text,
    fix_type text NOT NULL CHECK (fix_type IN ('server','computer','agent','report')),
    status text NOT NULL CHECK (status IN ('pending','applying','queued','waiting','done','failed','resolved','ignored','expired')),
    count int NOT NULL DEFAULT 1, first_seen_at timestamptz NOT NULL, last_seen_at timestamptz NOT NULL,
    quiet_seen_at timestamptz, ignored_until timestamptz, evidence jsonb NOT NULL DEFAULT '{}',
    action jsonb, before jsonb, applied_by text, applied_at timestamptz, waiting_since timestamptz,
    done_at timestamptz, proof_id text, error text
);
CREATE UNIQUE INDEX fixes_active_fingerprint_idx ON fixes (detector, fingerprint)
    WHERE status IN ('pending','applying','queued','waiting');
CREATE TABLE lookup_log (
    id bigserial PRIMARY KEY, person_id text NOT NULL, device_id text, session_id text,
    at timestamptz NOT NULL DEFAULT now(), kind text NOT NULL CHECK (kind IN ('search','get')),
    query text CHECK (char_length(query) <= 120), result_ids text[] NOT NULL DEFAULT '{}', hit boolean
);
CREATE INDEX lookup_log_session_at_idx ON lookup_log (session_id, at);
ALTER TABLE sessions ADD COLUMN nudge_asked boolean, ADD COLUMN nudge_saved boolean;
ALTER TABLE devices ADD COLUMN hook_error_at timestamptz,
    ADD COLUMN hook_error_class text CHECK (hook_error_class IN ('network','auth','server','parse')),
    ADD COLUMN outbox_depth integer;
ALTER TABLE memories ADD COLUMN faded_at timestamptz, ADD COLUMN edited_by text;
