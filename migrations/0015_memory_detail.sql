ALTER TABLE memories ADD COLUMN detail text NOT NULL DEFAULT ''
    CHECK (char_length(detail) <= 8000);

-- Keep the original generated vector and its index for rows with no detail.
CREATE INDEX memories_search_detail_idx ON memories
    USING gin ((search_tsv || to_tsvector('english', detail))) WHERE detail <> '';
