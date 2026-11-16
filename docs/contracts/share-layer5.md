---
id: CM-CONTRACT-010
title: "Layer 5 Explicit Project Memory Share"
status: draft
---

# Layer 5 Explicit Project Memory Share

This contract owns the first cross-person slice: a teammate joins an owner's project through
the committed project file and the owner explicitly shares one promoted project memory. The
rationale is [ADR 0011](../adr/0011-explicit-project-memory-share.md). Project registration
and promotion stay in [CM-CONTRACT-009](project-layer4.md).

A second self-hosted account is created after bootstrap with the same
credential body, by the admin's web session (see
[auth-layer3](auth-layer3.md#additional-accounts)):

```http
POST /cairn-memory/auth/register
```

Bootstrap remains the first account only. Register fails before bootstrap,
rejects a duplicate email, and returns the new account's session. The share target
must already exist and must be a member of the project.

Every owner route below accepts the owner's web session token or a device token
that belongs to the owner's account; a device token is not a member credential.
See the [permission table](auth-layer3.md#device-tokens-are-agent-credentials).

## Membership

There is no invite and no member removal. A teammate becomes a member when a capture or sync
push of theirs carries the owner's project id, which the hook and `cairn capture` read from the
committed `.cairn/project.json` (`cairn init` writes it); the rules are in
[project-layer4](project-layer4.md#membership-through-the-committed-project-file). Joining is
idempotent and writes the owner audit event `project_member_joined`. The routes
`POST /cairn-memory/projects/{projectId}/members`, `POST .../team/members` and
`POST .../team/members/remove` do not exist (`404`; a `POST` to `.../members` is `405`
because that path is now read-only). Because members cannot be removed, the
owner's control over what a member sees is to revoke shares.

```http
GET /cairn-memory/projects/{projectId}/members
```

The owner and any member (web session or device token) read the project's people, owner
first, then members by join time. It needs no team space, so it works for a project made by
`cairn init`. A person who is not a member gets `account is not a project member` (`400`),
an unknown project `unknown project: <id>` (`400`). Both stores answer the same:

```json
[
  { "person_id": "person-…", "email": "owner@example.test", "role": "owner", "joined_at": "1789000000" },
  { "person_id": "person-…", "email": "member@example.test", "role": "member", "joined_at": "1789000300" }
]
```

`joined_at` is unix seconds as a string (the project's `created_at` for the owner).

## Share

```http
POST /cairn-memory/projects/{projectId}/shares
Authorization: Bearer <owner-account-token>
```

```json
{
  "memoryId": "memory-project-…",
  "email": "member@example.test"
}
```

The memory must belong to that project, have project scope, and not be
forgotten, expired, or a personal observation. The member must already be
a project member (joined by a capture, see Membership). Repeating an active share of the same memory to the same member is
idempotent. The response uses `project_id`, `memory_id`,
`member_person_id`, `created_at`, and `revoked_at` when revoked.

A share does not copy the memory into the member's personal memories and does
not enqueue it for device sync pull.

## Member read

```http
GET /cairn-memory/projects/{projectId}/shared
Authorization: Bearer <member-account-token>
```

Any active session of a project member can list the active shared memories
for that project. Forgotten, expired, and revoked memories are omitted. A
shared memory that a live successor supersedes is omitted; the grant stays on
the original id until the owner shares the successor.

`GET /cairn-memory/projects` includes projects the account owns and projects
where that account is a member, so the member can open the shared read. Each
listed project includes `role`: `owner` or `member`. A project the account owns also
includes `share_policy`, the current policy (`manual` by default, or
`auto_nonsensitive_project_memory`, the strings `POST .../policy` accepts); it is omitted
for a member project. Both adapters report it (`MemoryStore::share_policy`,
`PostgresStore::share_policy`).
`GET /cairn-memory/projects/{projectId}/memories` stays owner-only and lists
only rows the owner owns or that have no owner, not a row another account owns.
Search and context for the member do not return the owner's private or unshared
records.

A shared memory object includes `share_reason` when it exists.
`share_reason` is `manual` for an owner share and
`auto_nonsensitive_project_memory` for a policy share. The object never
exposes the personal memory it was promoted from: `promoted_from` is `null`,
there is no `source_memory_id`, and `source_event_ids` is empty.

## Share policy

```http
POST /cairn-memory/projects/{projectId}/policy
Authorization: Bearer <owner-account-token>
```

```json
{
  "policy": "manual"
}
```

The default policy is `manual`. Manual promotion creates no share. The only
other policy is `auto_nonsensitive_project_memory`. While it is set, promoting
a project-scoped memory that is not secret and not sensitive creates an active
share to each current member and writes `project_memory_shared`. A private
observation, a secret, or a sensitive memory does not match. Turning the
policy back to `manual` does not revoke shares that already exist.

## Share candidates

```http
GET /cairn-memory/projects/{projectId}/share-candidates?email=member@example.test
Authorization: Bearer <owner-account-token>
```

The owner lists promoted project memories that are not currently shared with
that member, are not secrets, and are not personal observations. Each item has
`memory_id`, `reason`, and `not_auto_shared_reason`. Under the manual policy,
`not_auto_shared_reason` says the space policy is manual.

The owner accepts a candidate with the existing share route. Dismissing a
candidate does not create a share:

```http
POST /cairn-memory/projects/{projectId}/share-candidates/{memoryId}/dismiss
```

```json
{
  "email": "member@example.test"
}
```

A dismissed candidate leaves the list. A secret or private observation never
appears there and never becomes shared.

## Conflicts and revocations

```http
GET /cairn-memory/projects/{projectId}/conflicts
GET /cairn-memory/projects/{projectId}/revocations
```

The owner and the member can read both routes. A conflict record has
`memory_ids` for both live project memories the contradiction signal matches.
The owner sees conflicts among live project memories. The member sees
conflicts only among memories shared with that member. Neither memory is
deleted or hidden because the other exists. Revocations are share records
with `revoked_at` set. Each has `project_id`, `memory_id`, `member_person_id`,
`member_email` (the revoked member's account email, omitted if the account is
gone), `created_at`, and `revoked_at`. The owner already sees member emails in
the team list, so this adds no new exposure. A member sees only their own
revoked shares, with no memory content beyond the share record.

## Team space

The project cairn remains the shared space. A team space belongs to one
project. The only roles are owner and member.

```http
POST /cairn-memory/projects/{projectId}/team
Authorization: Bearer <owner-account-token>
```

```json
{
  "name": "Platform"
}
```

Only the owner can create the team. Repeating the same name returns the
existing team. A different name fails while that team exists. A member who
tries to create it receives the existing not-owner error.

```http
GET /cairn-memory/projects/{projectId}/team
```

The owner and a project member can list the team and its members. A person
who is not a project member cannot. Each member object has `person_id`,
`email`, and `created_at`.

The team's members are the project's members (without the owner), so the list fills as
teammates join; there are no team member add or remove routes. The team is the project's name for
its shared space and must be created before it can be listed; the `members` route above needs no team.

## Delete

```http
POST /cairn-memory/delete
Authorization: Bearer <owner-account-token>
```

```json
{
  "id": "memory-project-…"
}
```

Delete is owner-only for a memory the owner already stores. It hides that
memory from normal recall with the same tombstone as forget, keeps the record
in storage and export, and writes `memory_delete_retained`. A member delete of
a shared memory fails with the existing not-owner error. Delete does not purge
other devices. `id` accepts the same aliases as forget. Deleting or forgetting a
project copy affects only that copy and its shares: it never hides the personal
memory it was promoted from or that memory's source events, and forgetting the
personal memory does not hide the copy ([ADR 0009](../adr/0009-explicit-project-promotion.md)).

## Revoke

```http
POST /cairn-memory/projects/{projectId}/shares/{memoryId}/revoke
Authorization: Bearer <owner-account-token>
```

```json
{
  "email": "member@example.test"
}
```

Only the owner can revoke. A later member read omits the memory. The owner
still has it. Member correct and forget fail through the existing validation
path; a message containing "does not own" is 401.

## Audit

Owner audit history records `project_member_joined`,
`project_memory_shared`, `project_memory_share_revoked`,
`project_team_created`, `memory_promote`, `memory_forget`, and
`memory_delete_retained`. Metadata may contain the project id, member person
id, and memory id. It does not contain memory content, tokens, or passwords.

## CLI and MCP

The `cairn` binary and `cairn mcp` call the share routes above with the device token from
`cairn setup` (see [mcp-layer1](mcp-layer1.md)). The commands take no account token and no
store path. A second account is created in the web console, which also issues that
account's device token; the teammate joins the project by capturing in the committed
checkout ([project-layer4](project-layer4.md#membership-through-the-committed-project-file)).
An unknown email, a person who has not joined, or a non-owner action is an error. Each command
prints the server JSON and exits 1 with the server's message on an error:

- `cairn project-share <project-id> <memory-id> <email>`
- `cairn project-shared <project-id>`
- `cairn project-revoke <project-id> <memory-id> <email>`
- `cairn project-team-create <project-id> <name>`
- `cairn project-team <project-id>`
- `cairn delete-memory <memory-id>`

`delete-memory` prints nothing on success (the route returns `204`).

MCP tools take the camelCase fields below.

- `memory_project_share` (`projectId`, `memoryId`, `email`)
- `memory_project_shared` (`projectId`)
- `memory_project_revoke` (`projectId`, `memoryId`, `email`)
- `memory_project_team_create` (`projectId`, `name`)
- `memory_project_team` (`projectId`)
- `memory_delete` (`id`)

There is no invite, no team member add and no member removal, on the CLI, MCP or REST.

## Viewer

The `/viewer` Projects page keeps the account token used for project inspection. It reads `role` from `GET /cairn-memory/projects`. An owner page loads `GET /cairn-memory/projects/{projectId}/memories` and shows one member email field, **Share decision**, **Revoke share**, and **Delete memory** on each current decision. It also shows the team name and members and a team name field with **Create team**. Those buttons call the share, revoke, team, and delete routes and show an API error in the existing toast.

A member page loads `GET /cairn-memory/projects/{projectId}/shared` directly and does not request the owner memories URL. It lists the team name and members and renders shared memories with scope, verified commit, freshness, and supersedes. It does not show create, delete, share, or revoke.

The inspection page also shows **Current decisions**, **Handoffs**, **Stale memories**, **Conflicts**, and **Revocations**. A failed conflicts or revocations request shows the error in the toast and keeps the decisions already loaded. `api()` applies `Content-Type: application/json` after the other request options.

## Out of scope

Roles beyond owner and member, offline-device purge, encryption, federation,
and CRDTs remain out of this slice. Delete retains the stored record. Automatic
sharing exists only for the explicit `auto_nonsensitive_project_memory` policy
above. PostgreSQL stores the team in `migrations/0010_layer5_team_space.sql`.
