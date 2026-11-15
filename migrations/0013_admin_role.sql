-- The bootstrap (first) account is the only admin; only an admin web session creates
-- accounts. Dev databases: the earliest existing account becomes the admin.

ALTER TABLE cairn_accounts
    ADD COLUMN IF NOT EXISTS is_admin boolean NOT NULL DEFAULT false;

UPDATE cairn_accounts SET is_admin = true
WHERE person_id = (SELECT person_id FROM cairn_accounts ORDER BY created_at, person_id LIMIT 1)
  AND NOT EXISTS (SELECT 1 FROM cairn_accounts WHERE is_admin);

CREATE UNIQUE INDEX IF NOT EXISTS cairn_accounts_one_admin
    ON cairn_accounts ((true)) WHERE is_admin;

INSERT INTO cairn_schema_migrations (version)
VALUES ('0013_admin_role')
ON CONFLICT (version) DO NOTHING;
