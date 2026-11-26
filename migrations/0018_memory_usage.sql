ALTER TABLE usage
    ADD COLUMN shown integer NOT NULL DEFAULT 0,
    ADD COLUMN last_shown_at timestamptz,
    ADD COLUMN detail_opened integer NOT NULL DEFAULT 0;
