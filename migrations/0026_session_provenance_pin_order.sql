-- Session provenance is written only when a save or distill creates a memory.
-- Pin order is explicit; existing pins keep oldest-first order.

ALTER TABLE memories ADD COLUMN pin_position integer;

WITH ranked AS (
    SELECT memory_id,
           row_number() OVER (
               PARTITION BY scope, CASE WHEN scope = 'project' THEN project_id ELSE owner_id END
               ORDER BY pinned_at, memory_id
           )::integer AS pos
    FROM memories
    WHERE pinned_at IS NOT NULL AND scope IN ('personal', 'project')
)
UPDATE memories m
   SET pin_position = ranked.pos
  FROM ranked
 WHERE m.memory_id = ranked.memory_id;

CREATE UNIQUE INDEX memories_personal_pin_pos_idx
    ON memories (owner_id, pin_position)
    WHERE scope = 'personal' AND pin_position IS NOT NULL;

CREATE UNIQUE INDEX memories_project_pin_pos_idx
    ON memories (project_id, pin_position)
    WHERE scope = 'project' AND pin_position IS NOT NULL;

CREATE TABLE memory_sessions (
    person_id  text NOT NULL,
    session_id text NOT NULL,
    memory_id  text NOT NULL UNIQUE REFERENCES memories ON DELETE CASCADE,
    role       text NOT NULL CHECK (role IN ('saved', 'distilled')),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (person_id, session_id, memory_id),
    FOREIGN KEY (person_id, session_id) REFERENCES sessions
);
