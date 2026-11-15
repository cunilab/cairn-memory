---
id: CM-CONTRACT-008
title: "Layer 3 Self-Hosted Account Authentication"
status: draft
---

# Layer 3 Self-Hosted Account Authentication

This contract defines the first person-level authentication boundary. Humans
use the web console, which holds a signed-in web session (an account session
token). Agents use a device token that only that session creates, lists, and
revokes. The two credentials are separate; the permission table below says
which route accepts which.

## Account bootstrap

`POST /cairn-memory/auth/bootstrap` creates the first account only:

```json
{
  "email": "owner@example.test",
  "password": "correct horse battery staple"
}
```

The response contains an account record, an opaque session token, and its
expiry. Email identifiers are trimmed and ASCII-lowercased; an email is at most
320 bytes, has exactly one `@` with a non-empty part on each side, and has no
whitespace. Passwords must be 12 to
1024 bytes (bytes, not characters) and are never returned or stored in
plaintext.

After bootstrap, another bootstrap request fails with `400`. Bootstrap needs no
bearer token or operator secret: the first caller that reaches an empty store
creates the account. `GET /cairn-memory/health` reports `bootstrap_required`:
`true` while no account exists. The web console reads it and shows a "Create the
first account" form in place of sign-in; that form calls bootstrap through the
console proxy, which stores the session in the same httpOnly cookie as sign-in.

## Additional accounts

After bootstrap, `POST /cairn-memory/auth/register` takes the same body and
returns the same session result. It needs no bearer token, invitation, or
operator approval, so any caller that can reach the server can create an
account. This records implemented behavior and sets no policy. Before bootstrap
it fails with `account bootstrap has not completed`, and for an email that is
already registered with `account email is already registered`; both are `400`.
The CLI and MCP do not create accounts: account creation is a web action. The sharing
flow that uses a second account is in [share-layer5](share-layer5.md).

## Account sessions

| Operation | Endpoint | Authorization |
| --- | --- | --- |
| Login | `POST /cairn-memory/auth/login` | None; email and password body. A device bearer is `403` |
| Logout | `POST /cairn-memory/auth/logout` | Web session only |
| Current account | `GET /cairn-memory/auth/me` | Web session or device token |
| Audit history | `GET /cairn-memory/auth/audit` | Web session only; the caller's newest 100 events, file and PostgreSQL |

Account tokens are sent as:

```text
Authorization: Bearer <account-session-token>
```

Both adapters store only a SHA-256 hash of the account session token. Device
sync tokens are a separate credential; both adapters also store only a SHA-256
hash of the device token and look it up by hash.
Sessions expire after 30 days and logout makes the token unusable immediately.
Invalid account credentials and revoked or expired sessions do not return
account data.

Standard authentication statuses: a validation failure (an invalid email or
password, a second bootstrap, register before bootstrap, a duplicate email)
returns `400`; invalid login credentials, a missing bearer, and an unknown,
revoked, expired, or disabled account or device return `401`; a device token on a
web-only action returns `403` with `web console session required; device tokens
cannot do this`; the rate limit returns `429`. The server maps these from the error text (`account_api_error` in
`crates/server/src/lib.rs`, `PostgresError::is_unauthorized` in
`crates/storage/src/lib.rs`), so this list covers only these standard cases.

## Device ownership

`POST /cairn-memory/devices/register` requires a web session. `personId` is
optional; when present it must equal the session's `person_id`. The response
contains a device record and a separate opaque device token. The token appears
in that response only: list and read routes never return it. Registering a device
id that already exists for the same person issues a new token on both adapters
and the old one stops working. Listing, rotating
and revoking a device also need the web session, and the target device must
belong to the same person (`device is outside the caller scope`, `401`).

Account logout does not revoke device tokens; revoking a device in the web
console is the control for agent/device access. A revoked token is `401` on
every agent route and on sync.

## Device tokens are agent credentials

A device token resolves to the account that owns the device. Every agent route
below accepts it as it accepts a web session, with the same account scope: the
device sees and changes only that account's memory, projects, and shares.
The file-store router and the PostgreSQL router behave the same. Use of an
active device token records its `last_seen_at`, at most once per 60 seconds; the
file store keeps that value in memory until another write persists it. The sync
routes apply the same active-account check as the agent routes, so a device of a
disabled account gets `401` on `sync/push`, `sync/pull`, and `sync/diagnose`. A
`sync/pull` or `sync/diagnose` that moves no cursor writes nothing else.

| Group | Routes | Web session | Device token |
| --- | --- | --- | --- |
| Account and device management | `auth/bootstrap`, `auth/register`, `auth/login`, `auth/logout`, `auth/audit`, `devices/register`, `devices` (list), `devices/revoke`, `devices/rotate` | yes (`bootstrap`, `register`, `login` take no bearer; a device bearer on them is refused) | `403` |
| Merge confirmation | `projects/{id}/branches/{branch}/merge` | yes | `403` when the project has any member besides its owner (invited or team); allowed on a solo project |
| Identity | `auth/me` | yes | yes |
| Capture and sync | `session/start`, `session/end`, `observe`, `sync/push`, `sync/pull`, `sync/diagnose` | start, end: yes; observe: yes on the file store, `401` on PostgreSQL (device only, it is a sync push); sync: `401` (device only) | yes |
| Recall | `search`, `smart-search`, `context`, `recall`, `resume`, `unresolved`, `why-changed`, `diagnose` | yes | yes |
| Memory | `remember`, `correct`, `forget`, `delete`, `export`, `import` | yes | yes |
| Projects | `projects` (list and register), `promote`, `projects/verify`, `projects/{id}/confirm`, branches and worktrees (list, register), `branches/{branch}/close`, entities, `freshness`, `conflicts`, `revocations`, `maintenance`, `memories`, `shared` | yes | yes |
| Sharing | `members` (invite), `team` and `team/members` (create, add, list), `shares`, `shares/{memory}/revoke`, `policy`, `share-candidates` and its `dismiss` | yes | yes |
| Member removal | `team/members/remove` | yes | `403` |

The merge rule reads the owner's member and team-member records at the moment of
the call. A device that merged a solo project is refused as soon as a member is
added. Removing a member is web-only, so a device cannot empty a project to
reach the solo-merge path. In PostgreSQL the merge locks the project row first, so
a member invite that commits first is seen by the check and a later invite waits
for the merge. The web console session stays the way to confirm a merge on a
shared project.

## Agent clients: CLI and MCP

The CLI (`cairn`) and the MCP server (`cairn mcp`) are network clients of the routes in the
table above. `cairn setup <server-url>` (device token on stdin) stores the server URL and a device
token, created in the web console, in `$XDG_CONFIG_HOME/cairn/config.json` (mode `0600`);
`CAIRN_URL` and `CAIRN_DEVICE_TOKEN` together override it (one alone is an error). Every command and tool sends that token
as the bearer and nothing else; there is no account-token argument and no local-store mode.
The URL must be `https`, or `http` for a loopback host only. The client never follows a
redirect and never prints the token. A web-only route answers `403`, and the client shows
the server's message. Account creation, device management, and member removal have no
CLI command or MCP tool. The tool table is in [mcp-layer1](mcp-layer1.md).

## Limitations

The file-backed compatibility adapter remains available for local Layer 1/2
flows. Once that store has any account, session start, observe, session end,
and remember require an account bearer through the same `file_bearer` gate as
search, export, and `GET /cairn-memory/diagnose`. A missing or invalid bearer does not write. Observe, end,
and remember then use `capture_for_account`, `end_session_for_account`, and
`remember_for_account`. Start calls `start_session_for_account`. An account
file session is keyed by `(person_id, session_id)`. The public session id is
unchanged. The same account's later start returns the first context that
account stored. Another account with the same id gets a separate row and does
not receive the first account's context or `ended_at`. A session object with
no `person_id` is the unscoped row. Account start, empty-context capture, and
end do not adopt it. `STORE_VERSION` stays 1. Old observation context already
copied from a shared row is left as stored. Account summaries use
`memory-{person_id}-{session_id}` and include only that account's events.
Account export returns only that person's session rows. An account import
rejects a session `person_id` that is not the caller, and a session with no
`person_id` is stored as the caller's row. The same export imported by a
different person is rejected. Restoring two people's rows into one file is
the unscoped import, which merges on `(person_id, session_id)`.

Account import applies a forget tombstone only for an id the caller owns or an
observation or memory id the same bundle carries. A tombstone for an id another
person owns rejects the whole import, like a foreign row, and writes nothing.
Other ids are ignored. File ids are unique across the file and the first owner
keeps them, so only an owner can forget an id. Memory ids and event ids share
that id space: account capture and device sync push reject an event id that a
memory row of another person owns, with `event belongs to another person`. The
error shows that the id is owned, not by whom and not its content. A caller can
still reserve an unused id by importing a row under it, so an arbitrary id is not
guaranteed to be available; the other account sees a loud error, not a silently
dropped event. PostgreSQL scopes tombstones by person already. Details are in
[export-v1](export-v1.md).

In the file store an account forgets or deletes only an id it owns. An unowned
legacy id (data from before accounts, or written by an unscoped store call), a
foreign id and an unknown id all fail with `unknown memory or event: <id>`,
and nothing is written. A forget cascades a memory's `source_event_ids` into the
tombstone set only for source ids that same account owns, so a source id in a
row's metadata, including an id that is foreign, unknown or not yet captured, is
never authority and writes no tombstone, deletion record or audit entry for it.
`remember_for_account` records the account as owner of the source id it
generates, with the memory and in the same rename; no observation row is made.
That owner record, with or without a row, reserves the id: another account's
capture, device push or import of it fails with `event belongs to another
person`. Older generated source ids have no owner record and are not claimed, so
forgetting such a memory tombstones the memory id only. Such an old generated
source id is also unowned and has no row, so another account that already knows
the exact id can still capture it, and forgetting that account's own event can
hide the old memory from its owner's recall (nothing is deleted). This predates
the rule, is not repaired, and needs the id, which is not shown to other
accounts. The trusted unscoped forget (`MemoryStore::forget`, a library call with no
HTTP route behind a bearer) keeps cascading every source id.

Account import also requires every memory row's `project_id` to name a project the
caller owns; otherwise the whole import is rejected with `account does not own the
project`, whether the project is foreign, missing or unowned. The owner's project
list, share candidates, conflicts and maintenance omit a row another account owns
(an old row is kept, not deleted), so a foreign row cannot appear there or hide the
owner's memory.

A store with no accounts keeps the unscoped Layer 1 methods and their global
event-id idempotency. Those methods stay available after accounts exist:
`capture_observation` are library calls on the store file, not HTTP calls; the CLI and MCP
no longer reach them (they call the routes below with a device token). The HTTP routes
behind `file_bearer` are the account boundary. Unscoped export
returns every account's memories, observations, and forget tombstones; only its
sessions are limited to rows with no `person_id`. Unscoped session calls start,
inherit, and end only the row with no `person_id`. On
`capture_for_account`, a duplicate returns the stored
observation with `duplicate: true` only when the authenticated account already
owns the event id. Another account's event id, and an existing event id with
no owner, are validation errors. Those errors do not include the stored
observation and do not record a new owner. The file keeps one observation per
event id. A failed rename leaves neither the new observation nor its owner
record, and an owned duplicate does not write the file again. The hook posts
`sync/push` with a device token only (see [hook-v1](./hook-v1.md)). A file
device push also records the device's person as the observation owner. A file device can attach one sync event to an
observation this person already owns when that observation has no sync event.
Foreign event ids and unowned event ids that have a stored row stay rejected; an unowned id with no row is not. The cursor and `duplicate` flag
are in the [sync contract](./sync-layer3.md). The `cairn-storage` adapter and
`--database-url` server mode provide the PostgreSQL account, session, device,
observation, and sync-event boundary. That server registers
`POST /cairn-memory/observe` as a device-token sync push: it takes the observation
body, answers `{accepted, duplicate, observation}`, records the sync event like
`sync/push`, and gives a web session `401`, so the CLI and MCP capture works on both
servers. Session start and session end accept a web
session or a device token. `POST /cairn-memory/sync/push` accepts a device token
only. An account start
stores the redacted context. That start fills the row when merging the stored
object over the canonical empty context still equals that object, including
the column default `{}` and an omitted-field object such as `{"files":[]}`.
A real field or an extra key stays. The first non-empty context stays.
The push, duplicate, and summary-copy rules are in the
[sync contract](./sync-layer3.md). The hook delivers every capture by posting
`sync/push` with its device token. With `CAIRN_OUTBOX` unset it does not
enqueue. The opt-in device queue is the [hook contract](./hook-v1.md), which
owns the client rule.
Bootstrap, register, and login are each limited to 10 attempts per 60-second
window, per operation and per normalized (trimmed, ASCII-lowercased) email. The
counter lives in server memory and resets on restart, counts every attempt
including successful ones, and is not per IP; the 11th attempt in a window
returns `429`. `me`, `logout`, `audit`, and the device endpoints are not
limited. PostgreSQL records metadata-only audit events, among others, for
account bootstrap, register, login, and logout, device registration, resume,
rotation, and revocation, and sync push and duplicate operations, besides
project and share events. The file store records only project, share, team, promotion, and
forget or delete events, and no account or device events. Passwords,
tokens, and memory content are never written to the audit table. Password
recovery, MFA, and OIDC remain separate adapters because they require an
email/identity provider decision beyond the self-hosted Layer 3 boundary.

## Evidence

- `crates/core/src/lib.rs` implements password hashing, session hashing,
  expiration, persistence, and logout.
- `crates/server/src/lib.rs` exposes bootstrap, register, login, logout,
  current-account, audit, account-authorized device registration, and
  PostgreSQL Layer 3 routes, including the PostgreSQL `observe` route. The MCP tools and
  CLI commands are in `crates/client/src/` (`tools.rs`, `mcp.rs`, `bin/cairn.rs`).
- `crates/storage/src/lib.rs` implements the async PostgreSQL account, device,
  and sync repository.
- `migrations/0001_layer3_personal_memory.sql` defines the PostgreSQL boundary.
- `migrations/0002_layer3_audit.sql` defines metadata-only operational audit history.
- `crates/storage/tests/postgres.rs` records the provisioned-database flow.
- Existing tests that cover parts of this contract, listed by name and not run
  for this documentation change: in `crates/core/src/lib.rs`,
  `account_auth_hashes_credentials_and_persists_revocable_sessions`; in
  `crates/server/src/lib.rs`,
  `self_hosted_account_auth_bootstraps_logs_in_and_revokes_sessions`,
  `account_login_attempts_are_rate_limited` (login only),
  `device_registration_requires_account_ownership`, and
  `file_http_capture_uses_file_bearer_when_accounts_exist`.
  `scripts/smoke_postgres.sh` registers a second account and asserts an
  `account_bootstrap` audit event.
- Device tokens as agent credentials (2026-11-14): `authenticate_account`,
  `authenticate_session`, and `WEB_ONLY_ERROR` in `crates/core/src/lib.rs` and
  `crates/storage/src/lib.rs`; the 403 mapping, `bootstrap_required` health
  field, and device routes in `crates/server/src/lib.rs`. Tests:
  `device_token_is_an_agent_credential_scoped_to_its_account` (core),
  `file_device_token_is_an_agent_credential_and_web_only_routes_refuse_it`
  (server, file router), and the ignored
  `postgres_device_token_is_an_agent_credential_and_web_only_actions_refuse_it`
  (storage). The PostgreSQL test and `scripts/smoke_postgres.sh` were written
  but not run on 2026-11-14 because the disposable PostgreSQL container
  returned I/O errors; they compile under `clippy --all-targets`.
  `scripts/smoke_layer3.sh`, `scripts/smoke_web.sh`, and
  `scripts/smoke_hook_auth.py` run the same boundary over HTTP on the file
  router.
- The 30-day session expiry (`AUTH_SESSION_SECONDS`, checked on authentication
  in both adapters) is implemented in source but not verified: no test found
  builds an expired session. No test found covers the HTTP `400` for a second
  bootstrap, the password length bounds, or register's duplicate and
  pre-bootstrap errors.
- Network CLI and MCP (2026-11-14): `crates/client/tests/agent_flow.rs` runs the real
  `cairn` binary and `cairn mcp` on a device token against the file-store router, and,
  ignored, against the PostgreSQL router, including `403` on web-only routes and `401`
  after the device is revoked. Unit tests in `crates/client/src/lib.rs` cover the config
  file mode, environment override, loopback-only `http`, redirect refusal, and that the
  token is absent from output and errors.
