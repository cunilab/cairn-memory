-- Track memories pushed to an agent session (CM-SPEC §14.5) to enforce a shared
-- budget and prevent showing the same memory twice in one session.
CREATE TABLE session_shown (
    person_id  text NOT NULL REFERENCES accounts ON DELETE CASCADE,
    session_id text NOT NULL,
    memory_id  text NOT NULL REFERENCES memories ON DELETE CASCADE,
    source     text NOT NULL,                      -- brief, topic, recall
    shown_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (person_id, session_id, memory_id)
);
CREATE INDEX session_shown_at_idx ON session_shown (shown_at);
