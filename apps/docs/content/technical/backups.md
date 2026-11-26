---
id: CM-DOC-TECHNICAL-BACKUPS
title: "Backup and restore"
status: accepted
description: "Back up the Cairn PostgreSQL store and practice restoring it into a disposable target."
---

# Backup and restore

Back up the entire Cairn database before an upgrade. That includes the active Cairn schema, migration history and the pgvector extension in `public`. The web memory export is useful for transferring memories. A database backup also preserves accounts, projects, credentials, sessions, queues and skills.

## Create a backup

Use PostgreSQL client tools compatible with the source and destination. A reliable starting point for a drill is the same PostgreSQL major release and pgvector version as production. The destination needs pgvector installed on disk and a role that can create the extension in the restore database. Keep the Cairn release tag, PostgreSQL and pgvector versions, and configured `CAIRN_SCHEMA` with the archive. Configure access via a PostgreSQL service and password file, or equivalent, and keep dumps and credentials private.

```sh title="Shell"
umask 077
pg_dump --format=custom --file=cairn.dump --dbname='service=cairn-source'
pg_restore --list cairn.dump
```

Use a database dedicated to Cairn when possible. A full dump contains all objects in that database, including other schemas. PostgreSQL database roles and server-level settings are outside this backup. Recreate the Cairn database login and the required access on the target separately.

## Practice on a disposable target

Create a fresh restore database on an isolated PostgreSQL instance with pgvector installed. In these examples `service=cairn-drill` points to that **empty disposable database** and uses the role that should own the restored objects. Keep production clients and workers connected to their original database during the drill.

```sh title="Shell"
pg_restore --exit-on-error --single-transaction --no-owner --no-privileges \
  --dbname='service=cairn-drill' cairn.dump
```

The restore stops and rolls back if an object cannot be created. The connecting role owns restored application objects. If your deployment uses a separate service role, grant it the required access before starting Cairn.

Compare the source and restored database using the schema you configured. These queries use the default schema.

```sql title="Verify the restore"
SELECT extversion FROM pg_extension WHERE extname = 'vector';
SELECT version FROM cairn.schema_version ORDER BY version;
SELECT count(*) FROM cairn.projects;
SELECT count(*) FROM cairn.memories;
```

Compare counts to a quiet source or to counts captured while taking the backup. Start a separate Cairn server and console at the version that produced the backup. Set their database, schema and internal server URL to point to the isolated restore. Verify health, sign in, open a known project and record, and search for the record with a device attached to that test server. Only then test the planned upgrade against the restore and confirm that the same record is still available. Keep the original archive and the matching release until the migration is verified.

[Deployment variables](deployment.md#environment) · [Startup migrations](data-model.mdx#startup-migrations) · [Upgrade guide](../users/upgrade.md).
