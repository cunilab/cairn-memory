---
id: CM-CONTRACT-010
title: "Layer 5 Explicit Project Memory Share"
status: draft
---

# Layer 5 Explicit Project Memory Share

This contract owns the first cross-person slice: an owner invites one existing
account and explicitly shares one promoted project memory. The rationale is
[ADR 0011](../adr/0011-explicit-project-memory-share.md). Project registration
and promotion stay in [CM-CONTRACT-009](project-layer4.md).

A second self-hosted account is created after bootstrap with the same
credential body:

```http
POST /cairn-memory/auth/register
```

Bootstrap remains the first account only. Register fails before bootstrap,
rejects a duplicate email, and returns an account session. The share target
must already exist.

## Invite

```http
POST /cairn-memory/projects/{projectId}/members
Authorization: Bearer <owner-account-token>
```

```json
{
  "email": "member@example.test"
}
```

Only the project owner can invite. The email must belong to an existing
account, the owner cannot invite themselves, and an unknown email fails.
Repeating the same invite returns the existing membership. The response uses
`project_id`, `person_id`, `email`, and `created_at`.

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
invited. Repeating an active share of the same memory to the same member is
idempotent. The response uses `project_id`, `memory_id`,
`member_person_id`, `created_at`, and `revoked_at` when revoked.

A share does not copy the memory into the member's personal memories and does
not enqueue it for device sync pull.

## Member read

```http
GET /cairn-memory/projects/{projectId}/shared
Authorization: Bearer <member-account-token>
```

Any active session of the invited member can list the active shared memories
for that project. Forgotten, expired, and revoked memories are omitted. A
shared memory that a live successor supersedes is omitted; the grant stays on
the original id until the owner shares the successor.

`GET /cairn-memory/projects` includes projects the account owns and projects
where that account is a member, so the member can open the shared read.
`GET /cairn-memory/projects/{projectId}/memories` stays owner-only. Search and
context for the member do not return the owner's private or unshared records.

A shared memory object includes `share_reason` and `source_memory_id` when
those values exist. `source_memory_id` is the promoted source memory.
`share_reason` is `manual` for an owner share and
`auto_nonsensitive_project_memory` for a policy share. The memory's existing
`promoted_from` and `source_event_ids` stay on the object.

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
with `revoked_at` set. A member sees only their own revoked shares, with no
memory content beyond the share record.

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

Owner audit history records `project_member_invited`,
`project_memory_shared`, and `project_memory_share_revoked`. Metadata may
contain the project id, member person id, and memory id. It does not contain
memory content, tokens, or passwords.

## CLI and MCP

The file-backed `cairn` binary and the local MCP adapter call the same store
methods. Each share command and tool passes the account token through
`authenticate_account`. A missing token, unknown email, or non-owner action
is an error.

CLI commands print the store JSON and exit 1 on `StoreError`:

- `cairn account-register <store> <email> <password>`
- `cairn project-invite <store> <owner-token> <project-id> <email>`
- `cairn project-share <store> <owner-token> <project-id> <memory-id> <email>`
- `cairn project-shared <store> <member-token> <project-id>`
- `cairn project-revoke <store> <owner-token> <project-id> <memory-id> <email>`

MCP tools accept the camelCase fields below and the same names in snake_case.

- `memory_account_register` (`email`, `password`)
- `memory_project_invite` (`accountToken`, `projectId`, `email`)
- `memory_project_share` (`accountToken`, `projectId`, `memoryId`, `email`)
- `memory_project_shared` (`accountToken`, `projectId`)
- `memory_project_revoke` (`accountToken`, `projectId`, `memoryId`, `email`)

## Viewer

The `/viewer` Projects page keeps the account token used for project inspection. When `GET /cairn-memory/projects/{projectId}/memories` succeeds, the owner sees one member email field, **Invite member**, and **Share decision** and **Revoke share** on each current decision. Those buttons call the invite, share, and revoke routes and show an API error in the existing toast. When that memories request fails, the page loads `GET /cairn-memory/projects/{projectId}/shared` and renders those memories with scope, verified commit, freshness, and supersedes, without invite, share, or revoke controls.

The inspection page also shows **Current decisions**, **Handoffs**, **Stale memories**, **Conflicts**, and **Revocations**. A member still sees only shared memories and no owner controls. `api()` sends `Content-Type: application/json`.

## Out of scope

Roles beyond owner and member, offline-device purge, encryption, federation,
CRDTs, and delete remain out of this slice. Automatic sharing exists only for
the explicit `auto_nonsensitive_project_memory` policy above.
