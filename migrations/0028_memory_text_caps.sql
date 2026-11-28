-- Title and summary sizes 80 / 240 become suggestions; these are the hard caps. No row is rewritten.
ALTER TABLE memories
    DROP CONSTRAINT memories_title_check,
    ADD CONSTRAINT memories_title_check CHECK (char_length(title) <= 200),
    DROP CONSTRAINT memories_summary_check,
    ADD CONSTRAINT memories_summary_check CHECK (char_length(summary) <= 1000);
