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
uses a second account is in [share-layer5](../archive/contracts/share-layer5.md).

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

The flag lives on the account (`accounts.is_admin`), with at most one true row enforced by a unique partial
index in `migrations/0001_foundation.sql`. A database created before 0.1.6 is refused at startup, so there is
no backfill (pre-release, no compatibility path).

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
Use of an active device token records its `last_seen_at`, at most once per 60 seconds. The capture
routes apply the same active-account check as the agent routes, so a device of a disabled account gets `401`.

| Group | Routes | Web session | Device token |
| --- | --- | --- | --- |
| Account and device management | `auth/bootstrap`, `auth/register`, `auth/login`, `auth/logout`, `auth/audit`, `auth/password`, `auth/accounts` (list), `auth/accounts/{id}/password`, `devices/register`, `devices` (list), `devices/revoke`, `devices/rotate` | yes (`bootstrap` and `login` take no bearer and refuse a device bearer; `register`, `auth/accounts` and `auth/accounts/{id}/password` need the admin session) | `403` |
| Identity | `auth/me` | yes | yes |
| Capture | `session/start`, `session/end`, `session/checkpoint`, `session/brief`, `session/topic`, `observe` | start, end, checkpoint: yes; `observe`: `401` (device only) | yes |
| Recall | `search`, `get`, `session/brief`, `session/topic`, `graph`, `diagnose` | yes | yes |
| Memory | `remember`, `memories`, `feedback`, `forget`, `delete`, `contradict`, `supersede`, `links`, `export` | yes | yes |
| Projects | `projects` (list), `projects/init`, `projects/{id}/rename`, `projects/{id}/members`, `projects/{id}/memories`, `projects/{id}/branches`, `projects/{id}/conflicts` | yes | yes |
| Ownership transfer | `projects/{id}/ownership` | owner or admin, to an existing member | `403` |

Project memories are shared by scope: every member reads them and only the author forgets one.
Teams and selective-sharing routes are removed; see [project-layer4](project-layer4.md).

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

The server is PostgreSQL only; the file-backed adapter and the import and sync pull routes were removed in
0.1.6. `POST /cairn-memory/observe` takes a device token only; a web session gets `401`.
Session start, end and checkpoint accept a web session or a device token. The `cairn hook` command delivers every
capture by posting `observe` and keeps a device-bound outbox for outages; the
[hook contract](./hook-v1.md) owns the client rule.

An account forgets only a memory it owns; a foreign, forgotten or unknown id fails with `404` and nothing is
written. Memory ids are unique across the schema, so a save never reuses another person's id.

Bootstrap, register, login and password change are each limited to 10 attempts per 60-second window, per
operation and per normalized (trimmed, ASCII-lowercased) email. The counter lives in server memory and resets
on restart, counts every attempt including successful ones, and is not per IP; the 11th attempt in a window
returns `429`. `me`, `logout`, `audit`, and the device endpoints are not limited. The server records
metadata-only audit events (`audit_events`) for account bootstrap, register, login, logout, password reset and
change, device registration, rotation and revocation, and project init, rename and ownership transfer.
Passwords, tokens, and memory content are never written to the audit table. Password recovery, MFA, and OIDC
remain separate adapters because they require an email/identity provider decision beyond the self-hosted
Layer 3 boundary.

## Evidence

- `crates/core/src/account/` implements password hashing, token hashing and the account and device types.
- `crates/store/src/{account,device}.rs` implement bootstrap, register, login, sessions, password change and
  reset, device registration, rotation, revocation and the audit log over `migrations/0001_foundation.sql`.
- `crates/server/src/{auth,routes/account,routes/device}.rs` expose the routes and the `agent`, `device`, `web`
  and `admin` extractors; the `403` on web-only routes comes from `reject_device_bearer`.
- Tests: `crates/store/tests/account.rs`, `crates/server/tests/project_model.rs` and
  `crates/client/tests/agent_flow.rs` (the real `cairn` binary and `cairn mcp` on a device token, including `403`
  on web-only routes and `401` after the device is revoked). Unit tests in `crates/client/src/lib.rs` cover the
  config file mode, environment override, loopback-only `http`, redirect refusal, and that the token is absent
  from output and errors.
- `scripts/smoke_web.sh` runs the same boundary over HTTP; `crates/client/tests/hook.rs` runs the hook against a real server.
- The 30-day session expiry is implemented but has no test that builds an expired session.
