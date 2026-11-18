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

The bootstrap account is the admin (`is_admin: true`); every other account is not.
There is exactly one admin and no role-change API. `GET /cairn-memory/auth/me` returns
the account with `is_admin` (boolean) for a web session or a device token.

After bootstrap, `POST /cairn-memory/auth/register` creates an account from the same
body and returns the same session result. It needs the admin's web session
(`Authorization: Bearer <admin session>`); there is no open sign-up:

| Caller | Status |
| --- | --- |
| No bearer | `401` |
| Unknown, expired or revoked bearer | `401` |
| Device token (any account, including the admin's) | `403`, `web console session required; device tokens cannot do this` |
| Web session of a non-admin account | `403`, `admin web session required; only the admin account can do this` |
| Admin web session | `200` |

The returned session belongs to the new account, not the caller; the console does not
store it. Before bootstrap it fails with `account bootstrap has not completed`, and for
an email that is already registered with `account email is already registered`; both are
`400`. The authorization check runs before the per-email rate limit and before the email and
password are validated, so a caller without the admin session cannot use up an email's
attempts. The CLI
and MCP do not create accounts: account creation is a web action. The sharing flow that
uses a second account is in [share-layer5](share-layer5.md).

## Account administration and passwords

Three web-session routes manage accounts after bootstrap. A device bearer is `403`
(`web console session required; device tokens cannot do this`) on all of them, and no
bearer or an unknown, expired or revoked one is `401`.

`GET /cairn-memory/auth/accounts` needs the admin's web session (a non-admin session is
`403`, `admin web session required; only the admin account can do this`). It returns
every account, oldest first, without a password or hash:

```json
[
  {
    "person_id": "person-…",
    "email": "owner@example.test",
    "status": "active",
    "is_admin": true,
    "created_at": "1789000000"
  }
]
```

`created_at` is unix seconds as a string, like every other `created_at` in the API.

`POST /cairn-memory/auth/accounts/{person_id}/password` needs the admin's web session and
the body `{"password": "<new password>"}`. The new password follows the same rules as
register (12 to 1024 bytes). It answers `204`, replaces that account's password and revokes
**every** web session of that account, so it must sign in again; its device tokens stay
valid. An unknown `person_id` or a short password is `400`. The admin resetting its own
password ends its own sessions too. It writes the audit event `account_password_reset`
under the target account (metadata: `actor_person_id`).

`POST /cairn-memory/auth/password` takes any web session and the body
`{"current_password": "…", "new_password": "…"}`. A wrong current password is `401`
(`invalid account credentials`) and a new password that breaks the rules is `400`. Success
is `204`: the password changes and every **other** web session of the account is revoked;
the calling session and the account's device tokens stay valid. The route shares the login
rate limit (10 attempts per 60 seconds per normalized email, counted before the password is
checked, so a caller without a valid session cannot use up an account's attempts; the 11th is
`429`). It writes `account_password_change`. Passwords are never stored in plaintext or in
audit metadata. There is no password recovery for a locked-out admin: the admin resets the
other accounts, and nothing resets the admin's password without its own session.

Both adapters store the flag with the account (`Account.is_admin`; PostgreSQL
`cairn_accounts.is_admin`, at most one true row by a unique partial index,
`migrations/0013_admin_role.sql`). Development databases created before the column
exist treat the earliest account as the admin when the migration runs; a file store
written before the field has no admin and needs a new bootstrap (pre-release, no
compatibility path).

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
password, a second bootstrap, a duplicate email) returns `400`; invalid login
credentials, a missing bearer, register before bootstrap (the admin check runs
first and no session can exist yet), and an unknown,
revoked, expired, or disabled account or device return `401`; a device token on a
web-only action returns `403` with `web console session required; device tokens
cannot do this`; a non-admin web session on `auth/register` returns `403` with `admin web
session required; only the admin account can do this`; the rate limit returns `429`. The server maps these from the error text (`account_api_error` in
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
device sees and changes that account's private memory and the decisions of projects it belongs to.
The file-store router and the PostgreSQL router behave the same. Use of an
active device token records its `last_seen_at`, at most once per 60 seconds; the
file store keeps that value in memory until another write persists it. The sync
routes apply the same active-account check as the agent routes, so a device of a
disabled account gets `401` on `sync/push`, `sync/pull`, and `sync/diagnose`. A
`sync/pull` or `sync/diagnose` that moves no cursor writes nothing else.

| Group | Routes | Web session | Device token |
| --- | --- | --- | --- |
| Account and device management | `auth/bootstrap`, `auth/register`, `auth/login`, `auth/logout`, `auth/audit`, `auth/password`, `auth/accounts` (list), `auth/accounts/{id}/password`, `devices/register`, `devices` (list), `devices/revoke`, `devices/rotate` | yes (`bootstrap` and `login` take no bearer and refuse a device bearer; `register`, `auth/accounts` and `auth/accounts/{id}/password` need the admin session) | `403` |
| Record merge | `projects/{id}/branches/{branch}/merge` | yes, caller-owned branch | yes, caller-owned branch |
| Identity | `auth/me` | yes | yes |
| Capture and sync | `session/start`, `session/end`, `observe`, `sync/push`, `sync/pull`, `sync/diagnose` | start, end: yes; observe: yes on the file store, `401` on PostgreSQL (device only, it is a sync push); sync: `401` (device only) | yes |
| Recall | `search`, `smart-search`, `context`, `recall`, `resume`, `unresolved`, `why-changed`, `diagnose` | yes | yes |
| Memory | `remember`, `correct`, `forget`, `delete`, `contradict`, `supersede`, `similarity-reviews`, `export`, `import` | yes | yes |
| Projects | `projects` (list and register), `projects/init`, `projects/{id}/members` (list), `projects/verify`, `projects/{id}/confirm` (verify an existing project memory), branches and worktrees (list, register), `branches/{branch}/close`, entities, `freshness`, `conflicts`, `maintenance`, `memories` | yes | yes |
| Ownership transfer | `projects/{id}/ownership` | owner or admin, to an existing member | `403` |

Project decisions use the author/current-owner edit rule; all project members read
the same live decisions. Branch/worktree status is personal bookkeeping, so the
membership-based device merge gate is removed. Teams and selective-sharing routes
are removed; see [project-layer4](project-layer4.md).

## Agent clients: CLI and MCP

The CLI (`cairn`) and the MCP server (`cairn mcp`) are network clients of the routes in the
table above. `cairn setup <server-url>` (device token on stdin) stores the server URL and a device
token, created in the web console, in the selected config file (mode `0600`). File selection,
the ignored `CAIRN_URL` / `CAIRN_DEVICE_TOKEN` inputs, and the HTTP 401 hint are owned by
[mcp-layer1](mcp-layer1.md). Every command and tool sends that token
as the bearer and nothing else; there is no account-token argument and no local-store mode.
The URL must be `https`, or `http` for a loopback host only. The client never follows a
redirect and never prints the token. A web-only route answers `403`, and the client shows
the server's message. Account creation, password management and device management have no
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

Import strips publication state and restores every row as personal memory. A
`project_id` is not publication authority. Authenticated personal export excludes
published decisions; project state is read online. See [export](export-v1.md).

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
Bootstrap, register, login and password change are each limited to 10 attempts per
60-second window, per operation and per normalized (trimmed, ASCII-lowercased) email. The
counter lives in server memory and resets on restart, counts every attempt
including successful ones, and is not per IP; the 11th attempt in a window
returns `429`. `me`, `logout`, `audit`, and the device endpoints are not
limited. PostgreSQL records metadata-only audit events, among others, for
account bootstrap, register, login, logout, password reset and password change, device
registration, resume, rotation, and revocation, and sync push and duplicate operations, besides
project and share events. The file store records only project, share, team, promotion, password reset or change,
and forget or delete events, and no other account or device events. Passwords,
tokens, and memory content are never written to the audit table. Password
recovery, MFA, and OIDC remain separate adapters because they require an
email/identity provider decision beyond the self-hosted Layer 3 boundary.

## Evidence

- `crates/core/src/accounts.rs` implements password hashing, session hashing,
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
  for this documentation change: in `crates/core/src/accounts.rs`,
  `account_auth_hashes_credentials_and_persists_revocable_sessions`; in
  `crates/server/src/lib.rs`,
  `self_hosted_account_auth_bootstraps_logs_in_and_revokes_sessions`,
  `account_login_attempts_are_rate_limited` (login only),
  `device_registration_requires_account_ownership`, and
  `file_http_capture_uses_file_bearer_when_accounts_exist`.
  `scripts/smoke_postgres.sh` registers a second account and asserts an
  `account_bootstrap` audit event.
- Device tokens as agent credentials (2026-11-14): `authenticate_account`,
  `authenticate_session`, and `WEB_ONLY_ERROR` in `crates/core/src/accounts.rs` and
  `crates/storage/src/lib.rs`; the 403 mapping, `bootstrap_required` health
  field, and device routes in `crates/server/src/lib.rs`. Tests:
  `device_token_is_an_agent_credential_scoped_to_its_account` (core),
  `file_device_token_is_an_agent_credential_and_web_only_routes_refuse_it`
  (server, file router), and the ignored
  `postgres_device_token_is_an_agent_credential_and_web_only_actions_refuse_it`
  (storage). The PostgreSQL test and `scripts/smoke_postgres.sh` were written
  but not run on 2026-11-14 because the disposable PostgreSQL container
  returned I/O errors; they compile under `clippy --all-targets`.
  `scripts/smoke_postgres.sh`, `scripts/smoke_web.sh`, and
  `scripts/smoke_hook_auth.py` run the same boundary over HTTP.
- The 30-day session expiry (`AUTH_SESSION_SECONDS`, checked on authentication
  in both adapters) is implemented in source but not verified: no test found
  builds an expired session. No test found covers the HTTP `400` for a second
  bootstrap, the password length bounds, or register's duplicate and
  pre-bootstrap errors.
- Account administration (2026-11-16): `accounts`, `reset_account_password` and
  `change_password` in `crates/core/src/accounts.rs` and `crates/storage/src/lib.rs`; the routes
  in `crates/server/src/lib.rs`. Tests: `only_the_admin_session_lists_accounts`,
  `the_admin_resets_a_password_and_every_web_session_of_that_user_ends` and
  `changing_the_password_needs_the_current_one_and_keeps_only_this_session` (core),
  `file_admin_lists_accounts_and_resets_a_password` and
  `file_password_change_needs_the_current_password_and_is_rate_limited` (server), and the
  ignored `postgres_admin_lists_accounts_and_passwords_reset_and_change_revoke_sessions`
  (storage).
- Network CLI and MCP (2026-11-14): `crates/client/tests/agent_flow.rs` runs the real
  `cairn` binary and `cairn mcp` on a device token against the file-store router, and,
  ignored, against the PostgreSQL router, including `403` on web-only routes and `401`
  after the device is revoked. Unit tests in `crates/client/src/lib.rs` cover the config
  file mode, environment override, loopback-only `http`, redirect refusal, and that the
  token is absent from output and errors.
