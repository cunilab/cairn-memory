-- Processing (CM-SPEC-008 §2): origin of a memory, why it went stale, when its status last
-- changed, the last git check; module entities; the `link` job kind.
ALTER TABLE memories
    ADD COLUMN origin            text NOT NULL DEFAULT 'agent' CHECK (origin IN ('agent','system')),
    ADD COLUMN stale_cause       text CHECK (stale_cause IN ('feedback','missing')),
    ADD COLUMN status_changed_at timestamptz NOT NULL DEFAULT now(),
    ADD COLUMN checked_at        timestamptz;                      -- last git check
ALTER TABLE entities DROP CONSTRAINT entities_kind_check;
ALTER TABLE entities ADD CONSTRAINT entities_kind_check
    CHECK (kind IN ('file','module','symbol','command','test','service','term'));
ALTER TABLE jobs DROP CONSTRAINT jobs_kind_check;
ALTER TABLE jobs ADD CONSTRAINT jobs_kind_check CHECK (kind IN ('embed','distill','consolidate','link'));
CREATE INDEX memories_stale_idx ON memories (status_changed_at) WHERE status = 'stale';
