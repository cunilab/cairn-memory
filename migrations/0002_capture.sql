-- Typed captures (CM-SPEC-006 §2.4): what was captured, whether a command worked, and one row per
-- distinct content in a session.
ALTER TABLE observations
    ADD COLUMN kind text NOT NULL DEFAULT 'prompt' CHECK (kind IN ('prompt','edit','command','assistant')),
    ADD COLUMN ok   boolean;
CREATE UNIQUE INDEX observations_dedupe_idx ON observations (person_id, session_id, content_hash);
