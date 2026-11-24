-- Agent instructions (0.1.9): the session-start line and the Stop reminder can be edited in the web.
-- The global texts live in `server_state` (`agent_session_instruction`, `agent_stop_nudge`); a
-- project may override either, and NULL means "no override" (the global text, then the built-in one).
ALTER TABLE projects
    ADD COLUMN session_instruction text,
    ADD COLUMN stop_nudge          text;
