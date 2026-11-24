-- Times the same knowledge was saved. Existing rows start at 1.
-- source_id is the newer side of a supersede (see supersede()). origin 'auto' only:
-- a handoff chain uses origin 'agent' and must not add proof. Never lowers.
ALTER TABLE memories
    ADD COLUMN proof_count integer NOT NULL DEFAULT 1
        CHECK (proof_count >= 1);

UPDATE memories m
   SET proof_count = 1 + s.n
  FROM (
        SELECT source_id, count(*) n
          FROM links
         WHERE relation = 'supersedes' AND origin = 'auto'
         GROUP BY 1
       ) s
 WHERE m.memory_id = s.source_id
   AND m.proof_count < 1 + s.n;
