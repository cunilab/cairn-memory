-- Widen the named CHECK without rewriting device data.
ALTER TABLE devices
    DROP CONSTRAINT devices_agent_check,
    ADD CONSTRAINT devices_agent_check
        CHECK (agent IN ('claude', 'codex', 'opencode', 'grok', 'antigravity', 'hermes'));
