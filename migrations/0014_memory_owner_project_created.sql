CREATE INDEX IF NOT EXISTS memories_owner_project_created_idx ON memories (owner_id, project_id, created_at);
