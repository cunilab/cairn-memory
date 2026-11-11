---
id: CM-CONTRACT-009
title: "Layer 4 Project Registration and Promotion"
status: draft
---

# Layer 4 Project Registration and Promotion

## Project registry

Authenticated users register a repository as a personal project:

```http
POST /cairn-memory/projects
Authorization: Bearer <account-token>
```

```json
{
  "repository": "cairn-memory",
  "name": "Cairn Memory",
  "defaultBranch": "main"
}
```

Registration is idempotent for the same person and repository. Project
listing is available at `GET /cairn-memory/projects`.

## Repository branches and worktrees

Branches are registered under a person-owned project:

- `POST /cairn-memory/projects/{projectId}/branches`
- `GET /cairn-memory/projects/{projectId}/branches`

```json
{
  "name": "feature/cache",
  "baseBranch": "main",
  "baseCommit": "base123",
  "headCommit": "abc1234"
}
```

Worktrees are registered under a project and may reference a registered
branch:

- `POST /cairn-memory/projects/{projectId}/worktrees`
- `GET /cairn-memory/projects/{projectId}/worktrees`

```json
{
  "branchId": "branch-…",
  "path": "/worktrees/cache",
  "commit": "abc1234"
}
```

Registration is idempotent by project plus branch name or worktree path. An
unknown project or branch is rejected. These records are person-scoped and do
not grant access to another person.

Promoted decisions can be inspected at
`GET /cairn-memory/projects/{projectId}/memories`; forgotten or expired
decisions are omitted.

## Merge confirmation and branch closure

A registered branch can be explicitly confirmed as merged:

```http
POST /cairn-memory/projects/{projectId}/branches/{branchId}/merge
Authorization: Bearer <account-token>
```

```json
{
  "memoryId": "memory-…",
  "commit": "merge123",
  "reason": "Confirmed after the merge checks passed"
}
```

The source memory must belong to the registered branch and repository. The
operation creates or reuses the corresponding project decision, sets its
`verified_commit` to the merge commit with freshness `100`, changes the branch
status from `active` to `merged`, and records the decision link and redacted
reason. Worktrees attached to the branch become `removed`. Repeating the same
confirmation is idempotent; a different confirmation for an already resolved
branch is rejected.

An abandoned branch can be closed without promotion:

```http
POST /cairn-memory/projects/{projectId}/branches/{branchId}/close
Authorization: Bearer <account-token>
```

The request requires a reason, changes `active` to `closed`, records the
redacted reason, and removes attached worktrees. Merged branches cannot be
closed, and closed branches cannot be merged. Both transitions are
person-scoped and audited in PostgreSQL.

## Explicit promotion

```http
POST /cairn-memory/promote
Authorization: Bearer <account-token>
```

```json
{
  "projectId": "project-…",
  "memoryId": "memory-…",
  "reason": "Confirmed by the regression test"
}
```

The response is a new `decision` memory with `scope: project`. It includes:

- `project_id` and repository context;
- `promoted_from` source memory ID;
- original source event IDs;
- files, symbols, commands, and tests from the source evidence;
- redacted `promotion_reason`;
- durable retention.

The source memory remains unchanged. Branch, worktree, and commit fields are
removed from the promoted project context. Repeating the same promotion
returns the original promoted memory.

Secret, forgotten, expired, or repository-mismatched memories are rejected.
Project registration and promotion are person-scoped; shared memberships and
cross-person access remain Layer 5.

## Freshness verification

`POST /cairn-memory/projects/verify` refreshes a promoted decision against a
new commit:

```json
{
  "projectId": "project-…",
  "memoryId": "memory-project-…",
  "commit": "def5678"
}
```

Promotion stores the source commit as `verified_commit`. Retrieval with a
different current commit returns `stale: true` and an explanatory
`stale_reason`. Verification updates `verified_commit` and restores freshness
to `100`; it does not rewrite the source memory.

## Evidence entities

Active project entities are listed and registered on the same path:

- `GET /cairn-memory/projects/{projectId}/entities`
- `POST /cairn-memory/projects/{projectId}/entities`

```json
{
  "kind": "runbook",
  "key": "deploy",
  "content": "optional text",
  "sourceMemoryId": "memory-…",
  "commit": "abc1234"
}
```

`kind` and `key` are required. `content`, `sourceMemoryId` (alias
`source_memory_id`), and `commit` are optional. `GET` returns active entities
for that project. Registering the same kind and key again returns the existing
entity.

## Freshness history

`GET /cairn-memory/projects/{projectId}/freshness` returns the project's
freshness events. Each event uses `event_id`, `project_id`, `memory_id`,
`previous_commit`, `commit`, `action`, `reason`, and `created_at`.

## Maintenance listing

```http
POST /cairn-memory/projects/{projectId}/maintenance
Authorization: Bearer <account-token>
```

```json
{
  "currentCommit": "def5678"
}
```

`currentCommit` (alias `current_commit`) is optional. The response reports
`project_id`, `current_commit`, `stale_memory_ids`, `superseded_memory_ids`,
`active_memory_ids`, and `checked_at`. The call lists those ids and does not
change stored memories.

## Confirmation

```http
POST /cairn-memory/projects/{projectId}/confirm
Authorization: Bearer <account-token>
```

```json
{
  "memoryId": "memory-…",
  "commit": "confirm123",
  "reason": "Confirmed for the registry"
}
```

`memoryId` (alias `memory_id`) and `reason` are required. `commit` is
optional. The call promotes the source memory into the project and, when
`commit` is present, verifies that commit. The response is the project memory.

## Correction

`POST /cairn-memory/correct` corrects a memory, including a promoted project
decision. Linked correction history follows
[ADR 0010](../adr/0010-correction-keeps-linked-history.md): the new memory
sets `supersedes` to the old id and keeps the target `project_id` when the
target is a memory. Project listing returns the new memory while that
correction is live.

## CLI and MCP

The file-backed `cairn` binary and the local MCP adapter call the same store
methods. Each project command and tool passes the account token through
`authenticate_account`. There is no tokenless project API.

CLI commands print the store JSON and exit 1 on `StoreError`:

- `cairn account-bootstrap <store> <email> <password>`
- `cairn project-register <store> <account-token> <repository> <name> [default-branch]`
- `cairn project-list <store> <account-token>`
- `cairn promote <store> <account-token> <project-id> <memory-id> <reason>`
- `cairn project-memories <store> <account-token> <project-id>`
- `cairn project-freshness <store> <account-token> <project-id>`

MCP tools accept the camelCase fields below and the same names in snake_case.
A missing token or unknown project is a tool error.

- `memory_account_bootstrap` (`email`, `password`)
- `memory_project_register` (`accountToken`, `repository`, `name`, optional `defaultBranch`)
- `memory_promote` (`accountToken`, `projectId`, `memoryId`, `reason`)
- `memory_project_memories` (`accountToken`, `projectId`)
- `memory_project_freshness` (`accountToken`, `projectId`)

## Evidence

- `docs/adr/0009-explicit-project-promotion.md`
- `docs/adr/0010-correction-keeps-linked-history.md`
- `crates/core/src/lib.rs`
- `crates/core/src/bin/cairn.rs`
- `crates/server/src/mcp.rs`
- `scripts/smoke_layer4_cli.sh`
- `crates/storage/src/lib.rs`
- `migrations/0003_layer4_projects.sql`
- `migrations/0004_layer4_freshness.sql`
- `migrations/0005_layer4_project_entities.sql`
- `migrations/0006_layer4_branch_resolution.sql`
- `crates/server/src/lib.rs`
