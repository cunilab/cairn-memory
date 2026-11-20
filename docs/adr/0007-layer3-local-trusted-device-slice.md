---
id: CM-ADR-0007
title: "ADR-0007: Prove trusted-device sync before PostgreSQL and hosted authentication"
status: draft
---

# ADR-0007: Prove trusted-device sync before PostgreSQL and hosted authentication

- **Date:** 2026-10-26
- **Milestone:** Layer 3 — Personal Cross-Device Memory
- **Related:** [Roadmap](../roadmap.md), [Collaborative memory](../specs/collaborative-memory.md), [Layer 3 sync contract](../archive/contracts/sync-layer3.md), [ADR 0002](0002-collaborative-sync-model.md)

## Context

Layer 3 needs a real cross-device behavior, but the current implementation is
still a deliberately small file-backed Rust prototype. Introducing PostgreSQL,
OIDC, key rotation, encrypted caches, and a full edge client at once would make
it difficult to prove the core behavior and would overstate the security model.

## Decision

1. Add person-scoped device records with active/revoked status, last-seen time,
   and a durable sync cursor.
2. Use an opaque local prototype bearer token for the first trusted-device
   boundary; explicitly label it as non-production authentication.
3. Store accepted sync observations as person-scoped, cursor-addressable events.
4. Make push idempotent by `eventId`; make pull resumable by client cursor.
5. Add token rotation and same-person revocation before any sharing model.
6. Keep outage buffering in a bounded local JSON outbox that removes entries
   only after server acknowledgement and redacts common secrets locally.
7. Defer PostgreSQL, token hashing, OIDC, encrypted caches, cross-device purge
   guarantees, and production deployment until the local sync behavior is proven.

## Consequences

- The project has a real, testable cross-device flow without adding a database
  or authentication provider dependency.
- Duplicate delivery, cursor resume, token rotation, and revocation are visible
  and testable.
- A file-backed sync push publishes a new observation and its sync-event record
  in one store rename. If that rename fails, the push returns the IO error and
  leaves neither record in memory or in the file. A later push whose event id
  already has this person's sync event returns `duplicate: true` and that
  event's cursor. The response observation is the stored row. The request does
  not replace it. The rename is not followed by `fsync`.
- An existing file observation with no sync event gains exactly one sync event
  when its persisted owner record is the authenticated device's person. That
  first attachment returns `duplicate: false` and a new cursor: it is the first
  acceptance into the sync stream, so it is not the duplicate acknowledgement
  above. The sync event copies the stored observation. Session text, context,
  and a person id in the request are not ownership proof and do not replace
  the stored payload. The new sync event, cursor, and device last-seen time
  are one rename. If that rename fails, those additions are dropped and the
  existing observation and owner record stay, so a retry can attach the event.
  The next push then returns `duplicate: true` and the same cursor.
- An observation with no owner, and an observation owned by someone else, are
  rejected with no observation body and no new owner or sync event. Another
  person's existing sync event is rejected the same way. The file still has
  one observation per event id and resolves a sync event by that id. A new
  push with an empty context copies the device person's session row for that
  public session id, not a row with no person id and not another person's row.
- PostgreSQL still rejects a person-scoped observation that has no sync event
  and does not adopt that row. An empty device push, a context equal to
  `WorkContext::default()`, copies this person's existing session before
  normalize. It does not copy another person's row. If that session does not
  exist, the push inserts an empty session so the observation foreign key
  holds. A partial context is stored as sent and does not replace the session.
  An account start fills a session only while merging its stored object over
  the canonical empty `WorkContext` still equals that object, using the
  redacted request. The column default `{}` and an omitted-field object such
  as `{"files":[]}` are empty. A real field stays. An extra key stays with the
  row. The first non-empty context
  stays, including when two starts race. A later start does not change
  `ended_at` or rewrite observations or sync payloads. Session end applies the
  session context only to in-memory copies of observations whose context is
  still empty, and only while building the summary. There is no PostgreSQL
  `/cairn-memory/observe` route. Account tokens start and end sessions. Device
  tokens push. The hook's default remains unauthenticated file observe. An
  explicit account token is sent only to observe. An explicit device token is
  sent only to `sync/push`, which is the PostgreSQL capture path and also file
  device sync. With `CAIRN_OUTBOX` unset, the hook does not enqueue. With it
  set, only a retryable device failure is appended to the version 2 queue.
  Observe and account failures are not queued. Explicit replay stays an
  outbox command and does not read `CAIRN_DEVICE_TOKEN`; see the 2026-11-14
  amendment below for the hook's bounded pass.
  Do not send an account token to sync or a device token to observe.
- The local outbox redacts `content` and string items in the human-readable
  `WorkContext` lists (`files`, `symbols`, `commands`, `tests`, `services`,
  `handoffs`, `runbooks`) before it writes a new spool. It uses the same four
  markers, case-insensitive `sk-` token, and five recognized private-key PEM
  labels as core `redact_sensitive_content`. After a recognized BEGIN, base64
  glued to a truncated `-----END <label>` is removed with that footer token.
  An unclosed `BEGIN` and labels outside those five stay. Repository, branch,
  worktree, commit, ids, dates, and cursors stay, including a supported secret
  in those scalars. A missing or null `context` stays missing or null. A
  non-object `context` is rejected, and each present list among those seven
  names must be a list of strings. Otherwise `normalize` raises `ValueError`
  before `save`. The message names that field, or says the context must be a
  JSON object, and does not include the supplied value. A rejected enqueue
  does not create a queue file and does not change an existing one. Unknown
  keys are copied and are not validated. Identity scalars are not redacted.
  It does not rewrite an older spool and does not apply the sensitivity floor. Accepted server observations still go
  through core normalize. That path redacts the same text and keeps the
  stricter sensitivity label from the original content, concepts, and those
  lists. After the spool has redacted, the server classifies the redacted text
  and cannot see the original secret. That label limit is unchanged. Import
  redacts those lists plus `next_steps`, `unresolved_work`, and
  `promotion_reason`, and it keeps the bundle label. The outbox payload does
  not carry those import fields. Session start redacts the same lists and has
  no sensitivity field. Already stored rows and older spool files are not
  rewritten.
- Current version 1 replay fails closed on 301, 302, 303, 307, and 308. It
  does not request the redirect target or forward the device bearer. The old
  `urlopen` behavior followed 301, 302, and 303, including to another origin,
  and is not kept for compatibility. The event stays pending with its payload
  unchanged. Only a 401 purges the queue. A later queue version must fail
  closed the same way. Command output and the stored error omit the Location
  URL and the response body, including a non-http scheme, a missing Location,
  and a body that is short or still open. Replay closes that response without
  reading it. The rule is the [Layer 3 sync contract](../archive/contracts/sync-layer3.md).
- Version 1 `enqueue`, `replay`, and `purge` share one sibling `<queue>.lock`
  and hold `fcntl.flock` across the whole read, modify, and replace. `replay`
  keeps that lock during its socket calls. Dropping it early would let an
  enqueue land and then be overwritten. There is no lock timeout and no extra
  retry. Same-thread reentry is the lock inode, not the path spelling. `load`
  and `save` stay unlocked. `save` uses a unique temp file in the same
  directory and one replace. A failed replace keeps the previous queue. If
  `fcntl.flock` is unavailable the command does not write. This spool does not
  claim Windows locking. The behavior is the
  [Layer 3 sync contract](../archive/contracts/sync-layer3.md).
- Version 2 is an explicit device-bound queue, created only by `enqueue_bound`
  when the file is missing. It stores `route: sync/push`, a normalized server
  origin, and a SHA-256 fingerprint of a fixed prefix plus the raw device
  token. It does not store the token. A `%` in the hostname is rejected and
  not decoded, so an IPv6 zone id is unsupported. Version 1 enqueue refuses that file.
  Replay checks the origin, route, and fingerprint before any socket. A
  mismatch leaves the bytes unchanged and does not purge. A matching 401 still
  purges. One file is one token and one origin. Rotation fails closed: there
  is no rebind command, the new token does not match, and the old token can
  still purge on 401. The hook appends only a retryable device failure when
  `CAIRN_OUTBOX` is set. That preflight and append use `LOCK_NB` and do not
  wait while replay holds this lock across a socket call. A busy lock exits
  nonzero and does not retain the capture. The rule is the
  [Layer 3 sync contract](../archive/contracts/sync-layer3.md).
- Amendment, 2026-11-14: the hook replays opportunistically. Reason: queued
  events otherwise wait for a manual `cairn_outbox.py replay`. After the
  hook's own event returns 2xx, it takes the queue lock with `LOCK_NB` and
  skips silently if busy. Otherwise it replays the oldest pending events of
  the bound version 2 file through the existing replay code, at most 20
  events within a 2 second wall-clock budget, never purges (a 401 stops
  the pass), skips events with 3 failed attempts, and stops at the first
  transport failure. A busy lock at the hook's preflight is no longer fatal. The
  hook still starts no process or thread, never sends its own event twice,
  and a replay failure never changes its exit status or output. Explicit
  `replay` is unchanged, and the server deduplicates by event id. The rule is
  the [Layer 3 sync contract](../archive/contracts/sync-layer3.md#opportunistic-replay).
  This supersedes the earlier sentence that replay stays an explicit command.
- Amendment, 2026-11-14: the hook is device-token only. It always posts
  `sync/push` with the device credential. The 2026-11-14 `CAIRN_DEVICE_TOKEN`
  input is superseded for configuration by [ADR 0014](./0014-memory-scopes-distillation-and-hybrid-recall.md). The unauthenticated file observe
  default and the `CAIRN_ACCOUNT_TOKEN` observe path are removed, and the
  hook's offline refusal text for observe goes with them. A file `sync_push`
  of a new observation now records the device's person as owner so account
  reads see it. A device token is also accepted on the agent routes of the
  account that owns it, and device create, list, rotate and revoke need the web
  session ([ADR 0008](0008-self-hosted-account-authentication.md#amendment-2026-11-14-device-tokens-are-agent-credentials-the-web-session-is-the-human-surface)).
  This supersedes the sentences above that send an account token to observe and
  tell a device token not to be used as an observe credential, and the
  `device_id` self-service rotate and revoke by a device token.
- The local token implementation must not be presented as production security.
- The next Layer 3 slice can replace the storage and token adapter while
  preserving the sync contract and acceptance flow.

The next slice is now implemented by `cairn-storage`: PostgreSQL-backed
account sessions, hashed device tokens, canonical observations, sync events,
and cursor persistence preserve this contract through the server's
`--database-url` mode. Encrypted caches and purge guarantees remain deferred.

## Amendment 2026-11-15: a changed or out-of-scope replay is a conflict

Reason: before this change a push or observe that reused an `eventId` returned the
stored row as a duplicate whatever the request carried. A device could therefore
believe different content had been accepted when the server had kept the old row.
This changes a public contract: the sync response for a reused `eventId`.

1. A replayed `eventId` is identical only when its session, content (after
   redaction), role, and source match the stored row. An identical replay is still
   a duplicate: `duplicate: true`, the original cursor, and nothing written.
2. A replay that differs in any of those four fields returns `409 Conflict` with
   `event id already used with different content`. Nothing is written, adopted, or
   overwritten.
3. An `eventId` that already exists outside the caller's sync scope returns `409
   Conflict` with `event already exists outside sync scope`: on the file store an
   observation with no owner. (As first written this also covered a PostgreSQL
   observation with no sync event; see the difference note below.)
4. Both stores apply rules 1 and 2, and `POST /cairn-memory/observe` follows the
   same rules as `sync/push`.
5. The outbox treats `409` as a permanent rejection. It marks the event rejected,
   never resends it, and keeps it for inspection until it is purged.

Difference resolved 2026-11-15 (user decision): both stores adopt. When a person's
device pushes an event id that the same person captured through the account (an
observation with no sync event), the push is adopted as the same event if session,
content (after redaction), role, and source match: no second row, a sync event is
recorded, and later replays are duplicates. The first adoption returns `duplicate:
false`. Changed content returns `409` with `event id already used with different
content`, on the file store as well as PostgreSQL. Another person's id stays
rejected on the file store; PostgreSQL keeps a separate row per person. Rule 3's
`event already exists outside sync scope` now applies only to a file observation
with no owner.

The wire-level rules, status text, and examples are in the
[Layer 3 sync contract](../archive/contracts/sync-layer3.md). This ADR keeps its `draft`
status.
