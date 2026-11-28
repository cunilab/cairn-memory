-- Opt-in search trace for one person. Rows expire after 7 days.
-- The query is redacted and short. Results are ids, ranks and channel names, never memory text.

ALTER TABLE accounts ADD COLUMN retrieval_trace boolean NOT NULL DEFAULT false;

CREATE TABLE retrieval_traces (
    trace_id       text PRIMARY KEY,
    person_id      text NOT NULL REFERENCES accounts ON DELETE CASCADE,
    created_at     timestamptz NOT NULL DEFAULT now(),
    expires_at     timestamptz NOT NULL,
    query_redacted text NOT NULL,
    results        jsonb NOT NULL
);

CREATE INDEX retrieval_traces_person_idx ON retrieval_traces (person_id, created_at DESC);
CREATE INDEX retrieval_traces_expiry_idx ON retrieval_traces (expires_at);
