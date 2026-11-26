ALTER TABLE memories ADD COLUMN chat_id_hash text
    CHECK (chat_id_hash IS NULL OR (scope = 'personal' AND chat_id_hash ~ '^[0-9a-f]{64}$'));
