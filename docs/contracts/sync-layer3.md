---
id: CM-CONTRACT-007
title: "Layer 3 Trusted Device Sync"
status: draft
---

# Layer 3 Trusted Device Sync

This contract defines the first local Layer 3 slice: person-scoped trusted
devices, bounded authenticated replay, durable cursors, token rotation, and
revocation.

## Device identity

`POST /cairn-memory/devices/register` creates or resumes a device registration:

```json
{
  "deviceId": "device-macbook",
  "agentId": "agent-a",
  "name": "MacBook"
}
```

Only a signed-in web session creates a device. The body needs `name`.
`personId` is optional and defaults to the session's person. A different
`personId` is rejected with `account does not own the requested person`. A
device token calling this route gets `403` (see the permission table in the
[authentication contract](auth-layer3.md#device-tokens-are-agent-credentials)).
The response contains a device record and an opaque device token. That response
is the only time the token is shown: no list or read route returns it. The
device token is supplied as:

```text
Authorization: Bearer <device-token>
```

Both adapters store only a token hash. `deviceId` is optional (the server
generates `device-<nanos>` when it is absent) and `agentId` is optional. Resuming
a registered device of the same person issues a new token on both adapters, which
replaces the old one. Both adapters reject a revoked device with `device is revoked; register a
new device id` (`401`). A session acting on another person's device gets
`device is outside the caller scope` (`401`) on both. OIDC, key management, encrypted
caches, and offline purge guarantees remain later deployment work.

## Device operations

| Operation | Endpoint | Authorization |
| --- | --- | --- |
| Register/resume | `POST /cairn-memory/devices/register` | Web session; `personId` optional |
| List same-person devices | `GET /cairn-memory/devices` | Web session |
| Rotate a token | `POST /cairn-memory/devices/rotate` | Web session; target must be the same person's |
| Revoke a device | `POST /cairn-memory/devices/revoke` | Web session; target must be the same person's |

A device token on any of these gets `403`. Device records expose `status`,
`registered_at`, `last_seen_at`, and a durable sync cursor. `last_seen_at` is the
last use of the device token on any agent route (PostgreSQL writes it on each
authenticated call; the file store updates it in memory and persists it with the
next write). Revoking does not change it. Revoked devices receive `401` for
authenticated sync operations and for every other agent route.

## Sync operations

### Push

`POST /cairn-memory/sync/push` accepts the normal observation payload with an
active device token. Secret redaction and normal Layer 1 validation happen
before the event is added to the personal sync stream.

The response includes:

```json
{
  "accepted": true,
  "duplicate": false,
  "cursor": 1,
  "observation": {}
}
```

Pushing an `eventId` that already has this person's sync event returns
`duplicate: true` and that event's original cursor. It does not create another
observation or sync event. The observation in the response is the stored row.
A retry is identical when its session, content (after redaction), role, and source match
the stored row. A retry that differs in any of them is a conflict (`409`, `event id
already used with different content`) on both stores, and nothing is written; see
the store rules below. The same rule covers `POST /cairn-memory/observe`.

A new file-backed push also records the device's person as the observation
owner, in the same rename, so the account's search, session end, and export see
it as they see an observe. Before 2026-11-14 a pushed file observation had no
owner record; those rows are not backfilled.

A new file-backed push publishes the observation and its sync event in one
rename. A failed rename leaves neither, and that attempt does not return
`duplicate: true`. The rename is not followed by `fsync`. An empty context on
that new push copies the device person's `(person_id, session_id)` row. It
does not copy a row with no `person_id` or another person's row.

After a new push is acknowledged, both adapters register the context's project and
branch for the device's person and finish that person's idle sessions, as the account
`observe` does; see [continuity](continuity-v1.md#automatic-session-close). The file store
used to do neither on a push. A new file-backed push also creates the device person's
`(person_id, session_id)` row, with the pushed context, when none exists (the hook never
starts a session, and PostgreSQL does the same); a push or a session start with a real
context fills an empty row and never replaces a real one, and a failed rename leaves the row as it was. A housekeeping failure is logged and does not fail the
push. A duplicate push does neither.

On both adapters, an observation the person captured through the account (no
sync event yet) can gain exactly one sync event when that person's device pushes
the same event id with a matching session, content (after redaction), role, and
source. That is adoption: no second observation, a recorded sync event, and
later replays are the `duplicate: true` case above. The first attachment returns
`duplicate: false` and a new cursor, because no sync event existed yet. A push
whose session, content, role, or source differs is not adopted: `409`, `event id
already used with different content`, and nothing is written. On the file
adapter the persisted observation owner must be the authenticated device's
person. The sync event stores the canonical observation already on disk; the
request's context and any person text are not ownership proof and do not
replace that payload.
The sync event, cursor, and device last-seen time commit in one rename. A
failed rename drops those additions and leaves the observation and owner
record in place, so the same push can be retried. The following push is the
`duplicate: true` case above. After the first attachment, a pull for that
person returns the event once.

A file observation with no owner is rejected as a conflict (`409`,
`event already exists outside sync scope`). A file observation owned by someone
else, and another person's sync event, are still rejected as
`event belongs to another person` (`400`). Neither error returns the stored
observation or records a new owner or sync event. The file still keeps one
observation per event id. On the file store a differing replay, of an
observe or of a push before or after the sync event exists, is the same `409`.

PostgreSQL keys observations and sync events by `(person_id, event_id)`, so
another person's row with the same event id is a separate row, not a rejection.
A same-person duplicate returns the stored sync event and its original cursor.
The response observation is that stored row, and a pure replay writes no audit row.
A same-person observation with no sync event (an account capture or an import) is
adopted when the push matches it as above; the sync event and an audit row are
written in one transaction and the stored row is returned. A replay whose session,
content, role, or source differs from the stored event or observation is a conflict
(`409`, `event id already used with different content`); it is neither adopted nor
overwritten. `event already exists outside sync scope` is a file-store rule only. On PostgreSQL a device-token
`POST /cairn-memory/observe` is this same push, so it behaves alike. Session start and end accept a web session or a
device token. Sync accepts a device token only; a session token is `401`. The
hook always posts `POST /cairn-memory/sync/push` with `CAIRN_DEVICE_TOKEN`.
That post is redacted with the outbox normalizer before it is sent. With
`CAIRN_OUTBOX` unset, that post is not an enqueue and a failure is dropped.
With it set, only a retryable failure is appended to the version 2
queue below. After a delivered
device event, the hook makes one bounded, nonblocking replay pass over that
queue (see Opportunistic replay below); the explicit `cairn_outbox.py replay`
command is unchanged. The client rule is the [hook contract](./hook-v1.md).

An empty PostgreSQL push is a context equal to `WorkContext::default()`. When
this person's `(person_id, session_id)` row already exists, the push copies
that stored context before normalize. It does not copy another person's row. A context
that sets only `project_id` (what `cairn capture` sends) counts as empty for this copy and keeps
its `project_id`; the file store does the same on `observe` and `sync/push`.
When no session exists, the push inserts a session with the pushed context so the
observation foreign key holds, and a push with a real context fills an existing
empty row under the same rule as a start (a real context is never replaced). A partial context, including one that sets only `files`,
is stored as sent and does not replace the session. An account start may
replace a session context only while merging that stored object over the
canonical empty `WorkContext` still equals the canonical object, and only with
the redacted request. The column default `{}` and an object that only repeats
omitted default fields, such as `{"files":[]}`, are empty. A real repository,
branch, worktree, commit, or list value stays. An extra key stays with the
row, so the start does not erase it. The first non-empty context stays,
including when two starts race. A later start does not change `ended_at` and
does not rewrite observations or sync payloads. Session end applies the
session context only to in-memory copies of observations whose context is
still empty, and only while building the summary. The stored observation
stays as pushed.

### Pull

`POST /cairn-memory/sync/pull` accepts `{ "cursor": 0 }` and returns the
authorized events for the same person after that cursor. The file store returns
all of them. PostgreSQL returns at most 512 events and, separately, 512
deletion instructions per call (`MAX_PULL_EVENTS`). Events and deletions share
one cursor sequence, so a PostgreSQL page ends at the last cursor of a list that
hit its cap when that is lower than the other list's newest item: the page
contains only items at or below the returned cursor, and the rest arrive on the
next pull. The returned cursor is the value a client persists and sends on the
next pull. Repeating a pull with the returned cursor is empty.

PostgreSQL reads the event and deletion lists in one `REPEATABLE READ`
transaction, so a commit between the two reads cannot place an item below the
returned cursor that the page did not see. A cursor is allocated when its row is
inserted but is visible only at commit, so push and deletion take a
transaction-scoped advisory lock per person (`cairn-sync-cursor:<person_id>`)
before allocating and hold it to commit. One person's cursors therefore commit in
allocation order, and a pull never advances past a cursor that may still commit.
The cost is that one person's pushes serialize; other people are unaffected.

### Diagnostics

`GET /cairn-memory/sync/diagnose` accepts an active device token and reports
the device cursor, person stream cursor, lag, `pending_events`,
`rejected_events`, `status` (`caught_up` or `behind`), and a recovery hint. A
non-zero lag means the client should pull from its last persisted cursor. Both
adapters report `pending_events` and `rejected_events` as `0`; the server does
not track either. The local outbox `status` command reports `version`,
`pending`, `attempts`, `hasErrors`, `rejectedCount`, and `lastRejected` (see
Local outbox).

## Local outbox

`scripts/cairn_outbox.py` provides a bounded local spool:

```bash
printf '%s\n' '{"eventId":"evt-1", ...}' \
  | python3 scripts/cairn_outbox.py enqueue /tmp/cairn-outbox.json
python3 scripts/cairn_outbox.py replay /tmp/cairn-outbox.json \
  --url http://127.0.0.1:3111 --token '<device-token>'
python3 scripts/cairn_outbox.py status /tmp/cairn-outbox.json
```

The spool is bounded to 256 events. Before it writes a new queue file, it
redacts `content` and string items in a `context` object's `files`, `symbols`,
`commands`, `tests`, `services`, `handoffs`, and `runbooks` lists. The patterns
are the same as [lifecycle classification](lifecycle-v1.md): case-insensitive
`authorization: bearer `, `api_key=`, `token=`, and `password=`; a
case-insensitive `sk-` token that starts a token (not inside a word such as
`disk-full`); and a recognized private-key PEM block for
`PRIVATE KEY`, `RSA PRIVATE KEY`, `EC PRIVATE KEY`, `OPENSSH PRIVATE KEY`, or
`ENCRYPTED PRIVATE KEY`. That is not a general secret scanner. A header with
no matching `END` drops the header line and following base64 or OpenSSL
`Proc-Type` / `DEK-Info` lines. A line of optional PEM base64 glued to
`-----END <label>` without the closing `-----` is a footer: those glued bytes
and that footer token are removed, and text after the token stays. A later
line that is not a PEM continuation stays. A `BEGIN` that never closes with
`-----`, and labels outside these five, are not removed. Repository, branch,
worktree, commit, ids, dates, and cursors are not redacted. A missing or null
`context` stays missing or null. Any other context must be a JSON object, and
each present list among those seven names must be a list of strings. Otherwise
`normalize` raises `ValueError` before `save`. The message names that field,
or says the context must be a JSON object, and does not include the supplied
value. A rejected enqueue does not create a queue file and does not change an
existing one. Unknown keys are copied and are not validated. Identity scalars
are not redacted. The spool does not rewrite events already in an older queue
file, and it does not apply the sensitivity floor. Server normalize still redacts accepted observation content
and those lists, and it applies the sensitivity floor, as
[lifecycle classification](lifecycle-v1.md) describes. The server classifies
the payload it receives. After this client redacts, that floor cannot see the
original secret. The documented label limit is unchanged. The spool removes server-acknowledged events and 409 conflicts, and retains
transiently failed items with attempt/error metadata for retry or repair.
A `409` on replay (the server holds different content under that event id)
cannot succeed on resend, so the event leaves the queue in both `replay` and
the hook's bounded pass. `replay` lists it under `rejected` in its JSON summary
(`eventId` and `reason`), does not count it in `failed`, and exits 1 once. Both
paths add it to the queue file's `rejectedCount` and set `lastRejected`, which
`status` prints. A `401` during
replay means the device credential is revoked or invalid; the replay command
purges the queued events for that credential. Operators can also run
`cairn_outbox.py purge` explicitly. `replay` and `purge` on a missing queue
take no lock and create neither the queue nor its lock file. `replay` reports
`nothing to replay`; `purge` reports `{"purged": 0, "pending": 0}`.

Current replay, including a version 1 queue, posts only
`POST /cairn-memory/sync/push` and fails closed on `301`, `302`, `303`,
`307`, and `308`. It does not request the redirect target or forward the
bearer, and it does not retry that event in the same run. The event stays
queued with its payload unchanged and one added attempt. The stored error
and the command output omit the `Location` URL and the response body.
Replay closes that response without reading it. A `Location` whose scheme
is not `http`, `https`, or `ftp`, a missing `Location`, and a short or
still-open body use the same fixed error, `HTTP Error <code>: redirect
blocked`, with no header, URL, or body in the text.
The old version 1 `urlopen` behavior followed `301`, `302`, and `303` and
could send `Authorization` to another origin. Blocking that on the current
queue is a deliberate security fix, not a compatibility exception. A later
queue version must fail closed the same way. A `401` is still the only
replay response that purges the queue.

`enqueue`, `replay`, and `purge` take one exclusive advisory lock on the
sibling file formed by appending `.lock` to the queue path, using
`fcntl.flock`. The same thread may enter that lock again. Reentry is the lock
file's `(st_dev, st_ino)` after `open`, not the path spelling, so `/tmp` and
`/private/tmp` are one inode when `/tmp` is a symlink. The extra descriptor is
closed and the outer owner is not unlocked. `load` and `save` do not take the
lock, and `os.replace` keeps the queue path the caller passed. The lock file
is created mode `0600`, is never removed, and is not a queued event. A
rejected enqueue still does not create or change the queue file; it may leave
the lock file. `status` does not take the lock. It reads the last successful
replace.

The lock wait has no timeout. `enqueue` holds it while it reads stdin, checks
the id, and replaces the file, so an open stdin blocks the other two
commands. `replay` holds it across every network attempt and the final
replace. Each attempt uses the existing 10 second socket timeout, and the
queue holds at most 256 events, so one replay can hold the lock for that long
on every event it still has. There is no lock-wait retry and no background
replay. Releasing the lock before the replace would let an enqueue write an
event that the in-flight replay then overwrites.

`save` writes a unique temporary file in the queue directory and replaces the
queue once. A failed replace leaves the previous queue bytes in place and
removes that temporary file. An existing queue keeps its permission bits. A
new queue file is created mode `0600`. If `fcntl` cannot be imported, or
`flock` fails, the command raises `outbox locking requires Unix fcntl.flock;
refusing to update the queue unlocked` and does not write the queue. This
spool does not lock on Windows and does not write unlocked there. Windows
remains a packaging target for the server, CLI, and edge spool
([architecture](../architecture.md)). CLI `enqueue`, `replay`, and `purge`
still take a blocking lock. Replay still holds that lock across its socket
calls. The hook's preflight and append use `LOCK_EX|LOCK_NB` on the same
lock and do not wait. A busy lock is `outbox queue is busy`. The hook exits
nonzero, does not claim the capture was retained, and does not fall back to
an unlocked write. There is no sleep, no spin, and no background worker. A
caller that ignores the nonzero exit can drop the capture. The capture stays
unretained until a later hook run saves it.

### Opportunistic replay

Decision of 2026-11-14 ([ADR 0007](../adr/0007-layer3-local-trusted-device-slice.md)):
queued events otherwise wait for a manual command. With `CAIRN_OUTBOX` set,
after the hook's own event returns 2xx, the hook tries `LOCK_EX|LOCK_NB` on
the queue lock. A busy lock skips the pass silently. With the lock, it
replays the oldest pending events of that version 2 file through the same
`_replay_loaded` path as `replay`: the origin, route, and fingerprint check
before any socket, the redirect block, and `attempts`/`lastError` are
identical. The pass never
purges: an HTTP `401` stops it and adds an attempt, and only explicit
`replay` purges. It is bounded to 20 events and a 2 second wall-clock budget;
each request timeout is the budget left. Events with 3 or more attempts are
skipped and do not count against the 20; explicit `replay` still retries
them. The first transport failure, a bad acknowledgement, or an HTTP `401`,
`408`, `429`, or `5xx` stops the pass, and the remaining events stay pending
and unchanged.
An event whose id equals the hook's own event is not sent again. The pass
prints nothing, starts no process or thread, and any failure in it leaves the
hook's exit status, stdout, and stderr as the delivery set them. The server
deduplicates by event id, so a re-send is safe. The lock is held across these
socket calls, for at most the budget above. A concurrent hook sees a busy
preflight as non-fatal and still posts; only its retry append, if the post
fails, sees `outbox queue is busy` and does not retain the capture.

### Version 2 device binding

Version 1 `enqueue`, `replay`, and `purge` stay the unbound queue.
A missing `version` is version 1. `enqueue` refuses any other version, so it
does not append an unbound event to a version 2 file. Version 1 replay does
not read `CAIRN_ACCOUNT_TOKEN`. It does not follow redirects. A version 1 file
is not converted.

`enqueue_bound(path, event, origin, token)` creates version 2 only when that
path is missing. There is no new CLI command. The file is:

```json
{
  "version": 2,
  "route": "sync/push",
  "origin": "http://127.0.0.1:3111",
  "deviceFingerprint": "<sha256 hex>",
  "events": [{ "payload": {}, "attempts": 0, "lastError": null }]
}
```

`origin` is the server base URL, not a route: the origin plus an optional path
prefix, such as the `/agent` mount of a reverse proxy. Normalization removes
trailing slashes, lowercases the scheme and host, and omits the default port
(`80` for `http`, `443` for `https`); the path is kept as given.
`http://127.0.0.1:3111/` and `http://127.0.0.1:3111` are the same origin, and
`https://cairn.example.test/agent/` is stored as `https://cairn.example.test/agent`.
A queue stays bound to that exact string, so a different prefix is a different
server. `http` and `https` are the only schemes. A userinfo, query, or
fragment is rejected with the fixed message
`outbox origin must be an http(s) server base URL with no userinfo, query, or fragment`.
That message does not include the URL. A path with an empty, `.`, or `..`
segment, a `%`, or a `cairn-memory` segment is rejected with the same message,
so a base cannot already name the API route:
`http://127.0.0.1:3111/cairn-memory` is not accepted and does not become
`http://127.0.0.1:3111`. A hostname that contains `%` is rejected with that
same message and is not percent-decoded or stored. This includes `%2e`,
`%2E`, `%3a`, and `%00`. An IPv6 zone id, such as `fe80::1%lo0` or
`fe80::1%25lo0`, is unsupported. Bracketed IPv6 without a zone, such as
`[::1]`, stays valid. `localhost`, a trailing slash, and the default ports
still normalize as above. Replay does not follow a redirect to repair the
origin. Version 1 replay does not use this normalizer.

`deviceFingerprint` is the SHA-256 hex digest of the UTF-8 bytes of
`cairn-outbox-v2`, a NUL byte, and the raw device token. It is not the bearer
and it is not `hash_token`. The raw token is not written. `status`, `purge`,
errors, and the enqueue result do not print the fingerprint or the token.

The payload is `normalize(event)`. The 256 cap, duplicate `eventId`, content
and context-list redaction, and shape checks are the same as version 1. The
write is the same locked replace. A failed first create leaves no queue file
and no payload-bearing temporary file.

An existing version 1 file, a different origin, a different route, or a
different fingerprint is refused. Those bytes are not converted or overwritten.
One file is one device token and one origin. A second token uses another path.

Version 2 replay is the same `replay` command. It checks the account-token
rule, the metadata, the route, the origin, and the fingerprint before it
opens a socket. `CAIRN_ACCOUNT_TOKEN` is not a bearer and `CAIRN_DEVICE_TOKEN`
is not read. If the passed `--token` is exactly `CAIRN_ACCOUNT_TOKEN`, replay
exits before a socket and prints neither value. A mismatch, an unknown
version, or invalid metadata leaves the file byte-for-byte unchanged, does
not purge, and makes no request. `purge` of an unknown version, or of version
2 metadata that is not a route, origin, fingerprint, and event list, also
leaves the file unchanged. `status` reports the version and the pending
counts and does not print `deviceFingerprint`. The error text does not include
the token, the fingerprint, or the URL.

A `401` purges only after the origin, route, and fingerprint matched and the
POST returned `401`. The binding fields stay. Any other response uses the
version 1 rules, including the no-redirect opener. An accepted event,
including a server duplicate that returns `accepted: true`, is removed.
Remaining events keep their order.

Token rotation fails closed. The new token does not match the fingerprint, so
replay leaves the queue in place and does not purge. The old token still
matches, and a `401` from that old token still purges. There is no rebind
command and no silent rewrite of `deviceFingerprint`. Recovery is manual: keep
this file, and enqueue again into a new file with the new token. Do not point
two credentials or two origins at one queue.

The hook calls `enqueue_bound` only when `CAIRN_OUTBOX` is set,
`CAIRN_DEVICE_TOKEN` is the only credential, and the one POST was retryable.
It passes the normalized payload explicitly, with `nonblocking` and without
the enqueue report. `preflight_bound` is the check before that POST. It does
not create a missing queue file. Neither call is a replay, and replay does
not read `CAIRN_DEVICE_TOKEN`. A 401 purge still happens only after the
version 2 origin, route, and fingerprint matched. Observe offline enqueue is
not this queue. An empty queued context inherits this person's session at
accept time, not at hook time. A partial context stays as sent. Token
rotation still has no rebind command. A mismatched token fails closed and
does not purge. The old token can still purge on 401. Raw identity fields and
unsupported context keys stay unredacted. Older spool bytes are not rewritten.

## Safety and boundary

- Sync is person-scoped; a device cannot pull another person's events.
- Revocation stops future authenticated pull and push operations.
- Device records and tokens are not part of the Layer 1 memory export bundle.
- Clients may cache only redacted, bounded outbox payloads and the last
  acknowledged cursor. They must not persist account tokens in the outbox or
  cache server memory responses without an explicit encrypted-cache policy.
- A server forget or retained delete keeps the record in storage. Export still
  contains the forgotten record and its forgotten id. Normal recall hides it.
- Sync pull does not return an observation or memory the person has forgotten.
  A device that already pulled the item receives a deletion instruction on its
  next pull. A device that never pulls again is not purged.
- A revoked device must discard its token and queued outbox material, with
  repair requiring a newly authorized device token.
- The file-backed adapter proves local compatibility behavior. The PostgreSQL
  adapter provides the server-authoritative account, device, observation, and
  sync-event path.

## Evidence

- `crates/core/src/devices.rs` and `crates/core/src/sync.rs` store devices, cursors, sync events, rotation, and
  revocation.
- `crates/server/src/lib.rs` exposes the authenticated REST boundary.
- `crates/storage/src/lib.rs` implements the PostgreSQL Layer 3 repository.
- `migrations/0001_layer3_personal_memory.sql` defines the canonical tables.
- `crates/core/src/sync.rs` and `crates/storage/src/lib.rs` expose sync diagnostics.
- `scripts/cairn_outbox.py` implements the bounded local replay spool.
- `scripts/smoke_outbox.py` checks new queue-file bytes for `content` and the
  seven context lists, replay order, the 256 cap, duplicate event ids,
  401 purge, and that replay does not follow `301`, `302`, `303`, `307`, or
  `308`, without starting the server. It also checks that concurrent
  processes do not drop an enqueue or a purge, that replay holds the lock
  across its socket call, and that a failed replace keeps the previous queue.
  The version 2 checks cover a bound file with no raw token, binding failures
  that do not open a request, a matching 401 purge, accepted removal in order,
  and same-thread reentry through two paths to one lock inode.
  `scripts/smoke_hook_offline.py` checks the opt-in device hook: one POST,
  a redacted version 2 append, explicit duplicate replay, no observe queue,
  and a nonblocking lock. It does not start the Cairn server.
- `scripts/smoke_postgres.sh` proves registration, deduplication, pull,
  cursor lag, rotation, last-seen state, and revocation with a real PostgreSQL
  server. `scripts/smoke_outbox.py` proves enqueue redaction, offline replay,
  and purge of a revoked credential. `file_sync_adopts_an_owned_observation_once`
  in `crates/server/src/lib.rs` proves a duplicate event id from another person
  is rejected.
- `postgres_context_order_repairs_empty_session_without_rewriting_observations`
  calls `PostgresStore::start_session`, `sync_push`, `end_session`,
  `import_bundle`, and `export_bundle` on a fresh database. It covers empty
  push after start, empty push before start, the first non-empty start,
  another person's public id, a partial context, a duplicate cursor, and an
  imported observation that stays outside sync. It also plants a column-default
  `{}` session and a `{"files":[]}` session, checks a real branch and an extra
  key stay byte-for-byte, and checks the observation jsonb text is unchanged.
