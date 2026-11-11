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

`GET /cairn-memory/projects/{projectId}/memories` stays owner-only. Search and
context for the member do not return the owner's private or unshared records.

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

## Out of scope

Roles beyond owner and member, automatic sharing, offline-device purge,
encryption, and delete are not part of this slice.
