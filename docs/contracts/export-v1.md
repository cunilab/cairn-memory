---
id: CM-CONTRACT-003
title: "Cairn Export Format v1"
status: draft
---

# Cairn Export Format v1

The `cairn-memory` export is a portable JSON bundle for local backup, restore,
and migration. It copies the stored sessions, observations, and memories that
the export scope allows, plus forget tombstones. That includes temporary,
sensitive, expired, and forgotten rows, because a forget keeps the record. The
envelope has no credential field (account token, device token, provider key), but export
does not scan stored text. Content and the metadata lists named under
[Import behavior](#import-behavior) were pattern-redacted when a new row was
written, and import redacts them again. That redaction covers selected fields
only and is not an export guarantee. Ids, repository, branch, worktree, commit,
role, source, project and promotion links, and timestamps are exported as
stored, and a row stored before redaction covered a field is exported as stored.

## Envelope

```json
{
  "format": "cairn-memory",
  "version": 1,
  "exportedAt": "1771977600",
  "sessions": [],
  "observations": [],
  "memories": [],
  "forgottenIds": []
}
```

The session, observation, and memory entries use the serialized domain types.
Row fields are snake_case (`session_id`, `source_event_ids`), except that a
row's `lifecycle` object uses `expiresAt`. The envelope keys are camelCase
(`exportedAt`, `forgottenIds`). Memory and recall-related entries may include the
additive Layer 2 `scope` field (`session`, `worktree`, `branch`, `project`, or
`global`). Memory entries also include lifecycle, `kind`, `next_steps`,
`unresolved_work`, `project_id`, `promoted_from`, `promotion_reason`,
`verified_commit`, `source_event_ids`, and optional `supersedes` fields.
`forgottenIds` is the authoritative suppression set for imported state.

## Import behavior

- The format and version are validated before any state is changed.
- Event ID and memory ID are merge keys. A file session merges on
  `(person_id, session_id)`. A missing `person_id` is the unscoped row, which
  is the key older bundles already used. File-store account import rejects a different
  `person_id` and binds a missing one to the caller. That trust rule is in
  [the Layer 3 auth contract](auth-layer3.md). Unscoped import keeps the
  `person_id` from the bundle.
- Re-importing an identical bundle is a no-op and returns zero imported items.
- A same-ID, different-content conflict fails the complete import rather than
  silently replacing local state.
- Imported observation and memory content is redacted before it is persisted. The same redaction applies to session and record `WorkContext`
  lists (`files`, `symbols`, `commands`, `tests`, `services`, `handoffs`, and
  `runbooks`) and to memory `next_steps`, `unresolved_work`, and
  `promotion_reason`. Repository, branch, worktree, commit, ids, timestamps,
  and provenance links are unchanged. Redaction follows
  [lifecycle classification](lifecycle-v1.md). Import keeps the bundle's
  sensitivity label and does not apply the capture sensitivity floor. A kept
  `public` label is the bundle label after those strings were redacted. It is
  not a statement that the original metadata was safe. Already stored rows
  are not rewritten. Re-import compares the redacted bundle with the stored
  row. A stored row that still has the raw string conflicts instead of being
  rewritten.
- Size and shape limits reject the import and never truncate. Before any merge,
  in both stores, the whole import fails for a session, event, or memory id that
  is empty (an observation or memory also needs a non-empty `session_id`), an
  observation over 64 KiB or a memory over 256 KiB (measured before redaction),
  a lifecycle score above 100, or a lifecycle `expiresAt` that is not a decimal
  Unix-seconds string from `0` through `i64::MAX`. Import validation sets no explicit limit on sessions, lists, or row counts. The
  REST route inherits axum's default 2 MiB request-body limit (axum 0.8.9 per
  `Cargo.lock`), which this repository does not override (source-read, not run).
  Blank
  `forgottenIds` entries are dropped and duplicates collapse. Because these
  checks run before the merge, a rejection writes nothing. A file import that
  changes anything, and every file account import, writes once (one temporary
  file and one rename): the merged rows, the tombstones for rows that are
  already expired, and the account's ownership records go into that one write.
  If it fails, the in-memory state is restored and the file is unchanged. A
  failed save is covered by the regression tests (a blocked store directory);
  this is not a promise for power loss or `fsync`, and the file backend keeps
  no journal. A file account import that adds nothing still writes once. A
  file import that adds nothing writes nothing. PostgreSQL runs one transaction.
- Forget tombstones are merged and remain effective after restart.
- File-store account import applies a `forgottenIds` entry only for an id the caller already
  owns or for an observation or memory id that the same bundle carries. An id
  owned by another person rejects the whole import with the same ownership error
  as a foreign row, and nothing is written. Any other entry is ignored and not
  counted in `imported_forgotten_ids`, including a source event id that has no
  row in the bundle (a normal export lists one for each forgotten memory). A row's
  `source_event_ids` are not proof of ownership. A tombstone for an id that has no
  row and no owner is therefore not kept. The ownership error confirms that an id
  is owned by someone, with no payload or owner. Unscoped import (the library call) and
  a store with no accounts (REST: without a bearer) merge every entry. Tombstones already stored are not
  rewritten.
- File-store account import accepts a memory row that carries a `project_id` only when that
  project exists and the caller owns it. An outsider, an invited member, a missing
  project and a project with no owner record all reject the whole import with
  `account does not own the project`, and nothing is written. The bundle holds no
  project record, and import does not create a project, map one, or strip the
  link, so restoring into another store needs the matching project registered by
  the same account first. Unscoped import (the library call) and a store with no
  accounts (REST: without a bearer) keep the row as it is. PostgreSQL does not make this check (see the
  next bullet).
- PostgreSQL import runs in one transaction keyed to the authenticated person.
  It does not apply the file-store rules on session `person_id`, `forgottenIds`
  ownership, or `project_id` ownership. It ignores a bundle session's
  `person_id`, stores every `forgottenIds` entry as a tombstone for that person
  only, and does not check `project_id`. Read from source, not run: an
  observation needs its session in the bundle or already stored, and a memory id
  already held by another person fails the whole import in the insert, as a
  database error (HTTP 500), not with the ownership error. Format and version
  checks, redaction, and the limits above are shared with the file store. The
  same-ID conflict rule and the no-op re-import also exist in PostgreSQL, but
  existing tests assert only a zero memory count on re-import.
- A file-store account export lists a tombstone for an id the account owns (an
  owner record can exist with no row, such as a generated source id), for the id
  of an observation in the same export, and for any id named in `source_event_ids`
  of an exported memory, whether or not the account owns that id. This describes
  current behavior; it does not approve cross-account backup of negative
  knowledge, and whether the last branch should be narrowed is an open
  source-only residual. Restoring it into another store records ownership of the imported row
  ids only, never of a source id named in a row's metadata, so forgetting a
  restored memory there tombstones the memory id and the source ids that account
  owns, not an unowned source id. The trusted unscoped import and forget keep
  every id.
- Import does not call providers or replay capture hooks.

## Interfaces

- CLI: `cairn export <output>` and `cairn import <input>` (device token; the file is local,
  the call is the REST route below)
- REST: `POST /cairn-memory/export` and `POST /cairn-memory/import`
- MCP: `memory_export` and `memory_import`

Export returns the envelope itself. REST import takes `{"bundle": <envelope>}`,
and MCP `memory_import` takes the envelope as its `bundle` argument. The import
result keys are `imported_sessions`, `imported_observations`,
`imported_memories`, and `imported_forgotten_ids`. A file-store account
ownership rejection returns HTTP 401. PostgreSQL has no such rejection, and its
database errors return HTTP 500.

This remains a Layer 1/2 local format. Encryption, promotion, and cross-device
conflict policy are later concerns. File-store account import rejects a session
`person_id` that is not the caller's, as the
[Layer 3 auth contract](auth-layer3.md) states. An account export, file or
PostgreSQL, holds the memories and observations that person owns and the sessions
with that `person_id`; file tombstones follow the export rule above. The trusted
unscoped export (the library call, and the REST route on a file store with no
accounts and no bearer) holds every account's memories, observations, and
tombstones, and only the sessions with no `person_id`.
