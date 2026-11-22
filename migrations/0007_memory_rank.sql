-- Rank of a memory (0.1.9). Existing rows stay normal, which is today's behaviour.
-- New system notes are inserted as candidate; agent saves and handoffs stay normal.
ALTER TABLE memories
    ADD COLUMN rank text NOT NULL DEFAULT 'normal'
        CHECK (rank IN ('candidate', 'normal'));

-- Explicit no-op guard: the default already filled every existing row, including agent saves.
UPDATE memories SET rank = 'normal'
    WHERE rank IS DISTINCT FROM 'normal' AND origin = 'agent';
