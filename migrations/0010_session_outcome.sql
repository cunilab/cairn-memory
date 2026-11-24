ALTER TABLE sessions
    ADD COLUMN distilled_at timestamptz,
    ADD COLUMN notes_written integer,
    ADD COLUMN digest_written boolean,
    ADD COLUMN handoff_outcome text CHECK (handoff_outcome IN ('written','kept','skipped','none')),
    ADD COLUMN why_none text[],
    ADD COLUMN files_changed integer;
