CREATE TABLE IF NOT EXISTS cairn_jobs (
    job_id text PRIMARY KEY,
    kind text NOT NULL,
    job_key text NOT NULL,
    attempts integer NOT NULL,
    not_before bigint NOT NULL,
    claim_id text,
    lease_expires_at bigint,
    last_error text,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    finished_at bigint,
    succeeded boolean NOT NULL DEFAULT false
);

CREATE UNIQUE INDEX IF NOT EXISTS cairn_jobs_one_pending
    ON cairn_jobs (kind, job_key)
    WHERE succeeded = false AND claim_id IS NULL;

CREATE INDEX IF NOT EXISTS cairn_jobs_ready
    ON cairn_jobs (kind, not_before)
    WHERE succeeded = false;

CREATE TABLE IF NOT EXISTS cairn_job_quarantine (
    job_id text PRIMARY KEY,
    kind text NOT NULL,
    job_key text NOT NULL,
    attempts integer NOT NULL,
    last_error text NOT NULL,
    created_at bigint NOT NULL,
    failed_at bigint NOT NULL,
    quarantined_at bigint NOT NULL
);

INSERT INTO cairn_schema_migrations (version)
VALUES ('0016_background_jobs')
ON CONFLICT (version) DO NOTHING;
