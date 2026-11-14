-- A session with observations its summary may not cover. Set when a capture lands (or an
-- observation of the session is forgotten) and cleared when the summary is derived. The
-- automatic close reads only this partial index, so its cost does not grow with the number
-- of finished sessions.

ALTER TABLE cairn_sessions
    ADD COLUMN IF NOT EXISTS pending_since timestamptz;

CREATE INDEX IF NOT EXISTS cairn_sessions_pending_idx
    ON cairn_sessions (person_id, pending_since)
    WHERE pending_since IS NOT NULL;

INSERT INTO cairn_schema_migrations (version)
VALUES ('0012_session_summary_pending')
ON CONFLICT (version) DO NOTHING;
