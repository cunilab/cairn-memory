CREATE TABLE skills (
    skill_id         text PRIMARY KEY,
    name             text NOT NULL UNIQUE,
    current_version  integer NOT NULL DEFAULT 1 CHECK (current_version > 0),
    source_memory_id text UNIQUE REFERENCES memories ON DELETE SET NULL,
    archived_at      timestamptz,
    archived_by      text REFERENCES accounts
);

CREATE TABLE skill_versions (
    skill_id    text NOT NULL REFERENCES skills,
    version     integer NOT NULL CHECK (version > 0),
    title       text NOT NULL,
    description text NOT NULL,
    body        text NOT NULL,
    change_note text NOT NULL,
    author_id   text NOT NULL REFERENCES accounts,
    created_at  timestamptz NOT NULL DEFAULT now(),
    card_vector tsvector GENERATED ALWAYS AS (
        to_tsvector('english', title || ' ' || description)
    ) STORED,
    PRIMARY KEY (skill_id, version)
);

ALTER TABLE skills ADD FOREIGN KEY (skill_id, current_version)
    REFERENCES skill_versions (skill_id, version) DEFERRABLE INITIALLY DEFERRED;

CREATE INDEX skill_card_search ON skill_versions USING gin (card_vector);
CREATE INDEX skill_text_search ON skill_versions USING gin (
    (setweight(to_tsvector('english', title), 'A') ||
     setweight(to_tsvector('english', description), 'B') ||
     setweight(to_tsvector('english', body), 'D'))
);

CREATE TABLE skill_votes (
    skill_id  text NOT NULL,
    version   integer NOT NULL,
    person_id text NOT NULL REFERENCES accounts,
    vote      text NOT NULL CHECK (vote IN ('helpful', 'wrong')),
    note      text,
    PRIMARY KEY (skill_id, version, person_id),
    FOREIGN KEY (skill_id, version) REFERENCES skill_versions
);

CREATE TABLE skill_suggestion_dismissals (
    person_id text NOT NULL REFERENCES accounts ON DELETE CASCADE,
    memory_id text NOT NULL REFERENCES memories ON DELETE CASCADE,
    PRIMARY KEY (person_id, memory_id)
);

-- One skill per session, reserved under the same lock as memory cards.
CREATE TABLE session_skill_shown (
    person_id text NOT NULL REFERENCES accounts ON DELETE CASCADE,
    session_id text NOT NULL,
    skill_id text NOT NULL REFERENCES skills,
    shown_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (person_id, session_id)
);
CREATE INDEX session_skill_shown_at_idx ON session_skill_shown (shown_at);
