---
id: CM-ADR-0008
title: "ADR-0008: Use self-hosted accounts with hashed bearer sessions"
status: draft
---

# ADR-0008: Use self-hosted accounts with hashed bearer sessions

- **Date:** 2026-11-03
- **Milestone:** Layer 3 — Personal Cross-Device Memory
- **Related:** [Roadmap](../roadmap.md), [ADR 0003](0003-server-authoritative-postgres.md), [ADR 0007](0007-layer3-local-trusted-device-slice.md), [Layer 3 authentication contract](../contracts/auth-layer3.md)

## Supersession — 2026-11-16

[ADR 0013](0013-shared-project-decisions-and-personal-memory-scopes.md) supersedes
this ADR's membership-based device merge gate: branch bookkeeping is personal,
and devices can record their person's merges. Ownership transfer remains a web
session operation for the owner/admin. Account/device credential separation stays.

## Context

The local trusted-device slice uses opaque prototype device tokens and an
unauthenticated bootstrap route for device registration. The next Layer 3
slice needs a person-level identity boundary before a device can be attached to
that person. The first deployment target is self-hosted, and PostgreSQL is the
planned canonical store.

External OIDC would add an identity-provider dependency before the server's
basic account and device ownership behavior is proven. Supporting both models
at once would broaden onboarding, token, recovery, and test surfaces before
the personal-memory path is complete.

## Decision

1. The first production-oriented identity model is self-hosted accounts with a
   normalized email identifier and a password.
2. Passwords are stored only as Argon2id password hashes. The raw password is
   never persisted or included in responses.
3. Account sessions use opaque random bearer tokens. Only SHA-256 token hashes
   are persisted; sessions expire after 30 days and can be revoked.
4. The first account is created through a one-time bootstrap operation. The
   original implementation also exposed an unauthenticated register operation
   for later accounts; the 2026-11-15 amendment replaces it with admin-only
   registration (see the [contract](../contracts/auth-layer3.md)). Logout and current-account
   inspection use the account bearer token; login uses email and password.
5. Device registration requires an active account token and the requested
   `personId` must match that account. Device tokens remain a separate bearer
   credential for agent sync, rotation, and revocation.
6. OIDC, password recovery, multi-factor authentication, account invitations,
   and hosted identity federation remain later work.

## Consequences

- Account ownership is checked before a device is registered.
- Once the file store has any account, `POST /cairn-memory/session/start`,
  `/observe`, `/session/end`, and `/remember` use the same `file_bearer` gate
  as search and export. A missing or invalid bearer is rejected and does not
  write. Observe calls `capture_for_account`, end calls
  `end_session_for_account`, and remember calls `remember_for_account`. Start
  calls `start_session_for_account`. An account file session is one row per
  `(person_id, session_id)`. The public session id string is unchanged. The
  same account's second start keeps the first context. Another account with
  that id gets its own row. Account calls do not read or close an unscoped
  row, and unscoped calls do not read or close an account row. A file session
  written before this field has no `person_id` and stays the unscoped row.
  `STORE_VERSION` stays 1. Empty-context capture copies only the caller's row.
  A non-empty context is stored as sent. An account end summarizes that
  account's observations and takes scope from that account's session. An
  unscoped end summarizes only observations with no account owner. Account
  export includes only that person's session rows. Unscoped export includes
  only session rows with no `person_id`; its memories, observations, and
  tombstones are not filtered by owner. Account import rejects a session whose
  `person_id` is some other person and binds a missing `person_id` to the
  caller. Unscoped import keeps the stored `person_id` and merges on the
  pair, so two account exports can restore into one file. PostgreSQL already
  keys `cairn_sessions` by `(person_id, session_id)` and still treats the
  account token as the owner of an imported session. Each account's summary
  and search stay limited to that account's own events and memories.
- Account import applies a `forgottenIds` entry only for an id the caller owns
  or brings as a row in the same bundle. A foreign-owned id rejects the whole
  import with the row-ownership error. That error carries no payload or owner,
  but it does show that the id is owned by someone; an unknown id is accepted
  and ignored. Other entries, including source event ids with no row, are
  ignored. Unscoped import keeps every entry. This closes the case where one
  account hid another's memory, or silently poisoned a later capture, by
  importing a tombstone. Memory ids and event ids share one id space, so
  `capture_for_account` and device `sync_push` also reject an event id that a
  memory row of another person owns, with `event belongs to another person`.
  First owner still wins: a caller can bring a row under an unused id and so
  reserve it. The attempt then fails loudly for the other account; availability of
  an arbitrary id is not guaranteed. Old tombstones in an existing file are not
  identified or changed.
- Account import accepts a memory row with a `project_id` only for a project the
  caller owns. A foreign, missing or unowned project rejects the whole import with
  `account does not own the project` and writes nothing, so an outsider or
  member cannot add a row to another person's project. Import does not create or map
  a project or strip the link; a cross-store restore needs the matching project
  registered first. The owner-facing project views (list, share candidates,
  conflicts, maintenance row selection) skip a row another account owns, and apply
  the owner-aware supersession rule, without deleting it. Caller authorization of
  `project_maintenance` is decided below. PostgreSQL import is unchanged and not
  reviewed.
- In the file store an account forgets or deletes only an id it owns; an unowned
  legacy, foreign or unknown id fails with `unknown memory or event`. A forget
  cascades `source_event_ids` only for source ids that account owns, so metadata
  naming a foreign, unknown or future id never writes a tombstone. A generated
  explicit-memory source id is recorded as an owner record, with no observation
  row, and an owner record reserves the id against another account's capture,
  device push and import. Old data is not backfilled: legacy rows stay trusted
  local data and old unowned source ids are not claimed. Trusted unscoped forget
  keeps its cascade. PostgreSQL scopes forget by person already and is not
  reviewed here.
- File-store `project_maintenance` is owner-only: after authentication the caller
  must own a project that has an owner record, so a member, an
  outsider and an ownerless project are refused with `account does not own the
  project`. Members use explicit sharing, not a member-scoped maintenance report.
  An ownerless project is claimed by the first account that registers the same
  repository (`register_project` returns an ownerless project to any caller); that is
  not proof of the original owner, it predates this rule, and nothing repairs it
  automatically.
- File-store project registry operations are owner-only too: branch and worktree
  registration and listing, entity registration and listing, freshness history,
  branch merge confirmation and branch close use the same strict owned-project
  check after authentication, so a member, an outsider and an
  ownerless project are refused with `account does not own the project`. Merge
  confirmation also rejects a source memory another account owns, like
  promotion, and its existing-promotion lookup, like promotion's, considers only
  rows the project owner owns or that have no owner. Members use explicit sharing.
  PostgreSQL is not changed.
  Other Layer 4 registry endpoints (branches, worktrees, entities, freshness) are
  not changed here and stay queued. PostgreSQL is not changed.
- A file store with no accounts keeps the unscoped Layer 1 calls, including
  global event-id idempotency. `capture_observation`, the `cairn` CLI, and MCP
  stay on that unscoped key; the contract describes how these methods remain
  after accounts exist. `capture_for_account` still returns a duplicate
  only for an event that account already owns. A foreign or unowned event id
  is a validation error and does not return the stored observation or record
  a new owner. The file still has one observation per event id.
  `capture_for_account` writes a new observation and that account's owner
  record in the same rename. `capture_observation` writes an unscoped
  observation in one rename. If the rename fails, the staged rows are dropped
  and the error is returned, so a retry writes instead of acknowledging a
  duplicate. An owned or unscoped duplicate does not write the file again.
- `scripts/cairn_hook.py` posts observe with no bearer unless
  `CAIRN_ACCOUNT_TOKEN` is set. That account token is an observe bearer only.
  After an account exists, a hook with no token is unauthorized. There is no
  anonymous fallback and no second try with another credential. A device token
  is not an observe credential. A file device may attach one sync event to an
  observation that this person already owns and that has no sync event. Foreign
  and unowned event ids stay rejected. The cursor and `duplicate` flag are
  defined by the Layer 3 sync contract.
- The file-backed adapter can exercise the authentication contract without
  requiring a database. The `cairn-storage` adapter provides the async
  PostgreSQL account, device, observation, and sync-event boundary.
- Self-hosted operators own password and account recovery responsibilities.
- Device tokens and account sessions have separate scopes and revocation paths.
- A future OIDC adapter can map provider subjects to the same `person_id`
  without changing memory or sync ownership semantics.

## Amendment 2026-11-14: device tokens are agent credentials, the web session is the human surface

Reason: Cairn is automation-first. People use only the web console and agents
use a device token, so the two need separate credentials and a clear rule for
which route accepts which. The earlier model let an agent hold the account
bearer, which could not be told apart from a person.

1. Humans act in the web console with an account session token. Agents act with
   a device token that belongs to one account.
2. Only a web session creates (`devices/register`), lists, rotates and revokes
   devices. The token is returned once, at creation, and never by list or read.
   Rotate also needs the web session because it issues a token. The earlier
   device-authenticated list, rotate and revoke are removed.
3. A device token is accepted on every agent route and acts as its owning
   account, in the file-store router and the PostgreSQL router alike (the
   [permission table](../contracts/auth-layer3.md#device-tokens-are-agent-credentials)).
   A web session still works on those routes. Sync (`sync/push`, `sync/pull`,
   `sync/diagnose`) stays device-only.
4. A device token is refused with `403` and `web console session required;
   device tokens cannot do this` for account bootstrap, register, sign-in,
   sign-out, audit, device create, list, rotate and revoke, and for branch merge
   confirmation on a project that has any member besides its owner. A solo-project merge is allowed. A revoked device token is `401` on
   every route.
5. The first account is created from the web console on an empty server;
   `GET /cairn-memory/health` reports `bootstrap_required` so the console can
   show that form.
6. Hook: device token only (see [ADR 0007](0007-layer3-local-trusted-device-slice.md)).
   This supersedes the Consequences bullet that has the hook post observe with no
   bearer or with `CAIRN_ACCOUNT_TOKEN`, and the sentence there that a device
   token is not an observe credential.

Consequences: a compromised device token cannot create, list or revoke devices,
sign in, read the audit trail, or confirm a merge for a shared project, but it
can read, write, share, delete and export the account's memory until a person
revokes it. The CLI and MCP became network clients of these routes in the next
amendment. This ADR keeps its `draft` status.

## Amendment 2026-11-14 (second): the CLI and MCP are device-token network clients

Reason: the first amendment left `cairn` and the MCP adapter opening the store file
directly, with account-token arguments and local account creation. That is a second,
unscoped way into the data, and it asks an agent to hold an account credential. Humans
use only the web console; agents should use the same routes, with the same scoping, as
everything else.

1. `cairn` and the MCP server are REST clients (new crate `crates/client`; `cairn-core`
   stays free of HTTP and the server no longer carries MCP). The MCP entry point is
   `cairn mcp`; `cairn-server mcp --store` is removed.
2. The only credential is a device token. `cairn setup <server-url>` reads
   the token from stdin and stores it and the URL in the user config directory with mode
   `0600`; `CAIRN_URL` and `CAIRN_DEVICE_TOKEN` together override (one alone is an error). Local-file mode, store-path arguments, account-token
   arguments (including the MCP `accountToken`), and local account bootstrap and
   registration are removed. This supersedes the sentences above that keep the `cairn`
   CLI and MCP on the unscoped store calls.
3. Web-only actions have no command or tool: account and device management and member
   removal. A merge on a shared project has none either. If a client reaches such a route
   it gets `403` and shows the server's message.
4. The URL must be `https`; `http` is accepted only for a loopback host. The client never
   follows a redirect and never prints the token. TLS is rustls through one small blocking
   HTTP crate, `ureq`, chosen over `reqwest` because it needs no async runtime and no
   OpenSSL in the CLI.
5. The PostgreSQL router registers `POST /cairn-memory/observe` as a device-token sync
   push, so the same CLI and MCP capture works against the file-store and PostgreSQL
   servers. Other routes already matched.

Consequences: an agent host holds one revocable credential and cannot create accounts or
devices; the unscoped store methods remain library-only and are unreachable from the
agent tools. The cost is that the agent tools need a running server; there is no offline
mode. A token passed to `setup` is visible in the process list while it runs; the
environment variables avoid that.

## Amendment 2026-11-15: admin role and admin-only account creation

Reason: the first amendments left `POST /cairn-memory/auth/register` open to any caller
that can reach the server, which the ADR itself recorded as implemented behavior and not
approved policy. A self-hosted server needs no open sign-up, and the automation-first
model needs one person who administers accounts.

1. The bootstrap (first) account is the admin: `Account.is_admin` is `true` for it and
   `false` for every other account. There is exactly one admin for now and no
   role-change API. `GET /cairn-memory/auth/me` returns `is_admin`. PostgreSQL stores it
   in `cairn_accounts.is_admin` with a unique partial index that allows one true row
   (`migrations/0013_admin_role.sql`); the file store stores it in the account record.
2. `POST /cairn-memory/auth/register` requires the admin's web session. No bearer is
   `401`; a device token (any account) is `403` with the existing web-only text; a
   non-admin web session is `403` with `admin web session required; only the admin
   account can do this`. The admin supplies the new account's email and password; the
   response shape is unchanged, and its session belongs to the new account, so the
   console does not store it. This supersedes decision 4's open register.
3. Nothing else about the model changes: accounts are still person-scoped, and the admin
   has no access to another person's memory.

Consequences: account creation is a web action by one person. An admin who loses the
password has no recovery path (password recovery is still open). Multiple admins and
role changes need a later amendment. A file store or development database that predates
the field: PostgreSQL promotes the earliest account when the migration runs; a file
store has no admin until it is recreated (pre-release, no compatibility path). This ADR
keeps its `draft` status.

## Security boundary

This ADR defines the first account contract and storage shape. The PostgreSQL
adapter is the first server-authoritative Layer 3 deployment path; a complete
production deployment still needs secret rotation, durable rate limiting (the
current limiter is in server memory and keyed by operation and email), password
recovery, account and device audit records in the file store (PostgreSQL
records them), encrypted caches, advanced retrieval parity, and offline purge
guarantees.
