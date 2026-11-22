-- Ways to ask for a memory. Derived rows are copies of text the memory already has.
-- `learned` is reserved for an admin apply (Fixes F11); this migration writes none.
CREATE TABLE memory_aliases (
    memory_id text NOT NULL REFERENCES memories ON DELETE CASCADE,
    ord smallint NOT NULL,
    source text NOT NULL CHECK (source IN ('agent', 'derived', 'learned')),
    text text NOT NULL CHECK (char_length(text) BETWEEN 1 AND 120),
    embedding vector(384),
    embedding_source text,
    PRIMARY KEY (memory_id, ord)
);
CREATE INDEX memory_aliases_hnsw ON memory_aliases USING hnsw (embedding vector_cosine_ops);

INSERT INTO memory_aliases (memory_id, ord, source, text)
SELECT memory_id, 100, 'derived', title
  FROM memories
 WHERE status <> 'forgotten'
ON CONFLICT DO NOTHING;

INSERT INTO memory_aliases (memory_id, ord, source, text)
SELECT memory_id, 101, 'derived', left(fields->>'symptom', 120)
  FROM memories
 WHERE status <> 'forgotten'
   AND kind = 'gotcha'
   AND coalesce(fields->>'symptom', '') <> ''
ON CONFLICT DO NOTHING;
