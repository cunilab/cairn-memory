-- Allow 'system' as link origin (for system-written handoff chain links).
-- Relabel existing handoff-chain supersedes links written by the system.
ALTER TABLE links DROP CONSTRAINT links_origin_check;
ALTER TABLE links ADD CONSTRAINT links_origin_check
    CHECK (origin IN ('agent','auto','system'));

UPDATE links l
SET origin = 'system'
FROM memories s, memories t
WHERE l.source_id = s.memory_id
  AND l.target_id = t.memory_id
  AND l.relation = 'supersedes'
  AND l.origin = 'agent'
  AND s.kind = 'handoff'
  AND s.origin = 'system'
  AND t.kind = 'handoff'
  AND t.origin = 'system';
