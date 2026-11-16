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

`cairn init` is the one-call form for a repository checkout:

```http
POST /cairn-memory/projects/init
Authorization: Bearer <account-token>
```

```json
{ "repository": "cairn-memory", "name": "Cairn Memory" }
```

`name` is optional and defaults to the repository (at most 512 bytes, like
the repository). The call creates a project owned by the caller for that repository, or returns
the caller's existing one, and answers exactly:

```json
{ "project_id": "project-…", "repository": "cairn-memory", "name": "Cairn Memory", "created": true }
```

`created` is `false` when the caller already owned the project (the stored name is kept). A web
session or a device token may call it; both stores behave the same. A member of someone else's
project for the same repository still gets (or creates) their own project, so run it only to
start a new project; a teammate joins by capturing in the committed checkout (below).

## Membership through the committed project file

`cairn init` writes `<git toplevel>/.cairn/project.json` from that response:

```json
{
  "project_id": "project-…",
  "repository": "cairn-memory"
}
```

It holds no secret and no server URL. Committing it is how a team shares the project: there are
no invites and no member removal ([ADR 0011](../adr/0011-explicit-project-memory-share.md#amendment-2026-11-16-membership-through-a-committed-project-file-no-invites-no-removal)).
The repository name is the hook's git-derived one (the last path part of `origin`, else the
top-level directory name). `cairn init` refuses to overwrite a file that names a different project
id (exit 1, the file is unchanged) and tells the user to commit the file.

The hook ([hook-v1](hook-v1.md)) and `cairn capture` read that file from the git top level of the
working directory and send its id as `context.project_id`. When a capture or sync push (file store
`capture_for_account` and `sync_push`, PostgreSQL device `sync/push`) carries a `project_id` of an
existing project that another person owns, the capturer becomes a member of it, idempotently, and
the owner's audit history records `project_member_joined`. The stored observation keeps
`context.project_id`. A member's capture then never creates a private project for that repository
(automatic registration below is skipped), even when the member's repository name differs. An unknown
id, a project with no owner record, and the capturer's own project are ignored: the capture still
succeeds and falls back to the repository rules below. A context that carries only `project_id`
still inherits the started session's other context fields.

Anyone with an account on the server who captures with a project id joins that project. A member
reads only what the owner explicitly shares ([share-layer5](share-layer5.md)), so the id is not a
secret in the sense of memory access, but a member cannot be removed; the owner's control is to
revoke shares. Everyone can see who is in a project with `GET /cairn-memory/projects/{projectId}/members`
([share-layer5](share-layer5.md#membership)).

## Capture registers the project

A capture does the same without a call. When an account-owned observation (file
store `capture_for_account`, PostgreSQL device `sync/push`) carries
`context.repository`, the trimmed repository selects a project: one the person
owns first, then one the person is a member of, otherwise a new project (name is
the repository) owned by the capturing person is created. A project with no owner
record is never used or claimed. The branch is then registered on the owned
project, idempotently; an empty branch or `HEAD` is skipped, and a member of a
shared project registers no branch (branch writes are owner-only). A failure is
logged and does not fail the capture.

Registration is idempotent for the same person and repository. Project
listing is available at `GET /cairn-memory/projects`. In the file store it lists
the projects the caller owns and the projects shared with the caller as a member;
the `role` field is in [share-layer5](share-layer5.md).

## Repository branches and worktrees

In the file store the project registry operations are owner-only, for reads and
writes: branch and worktree registration and listing, evidence entity
registration and listing, freshness history, branch merge confirmation and branch
close. After authentication the caller must own a project that has an owner
record. A current member and a registered outsider who know the
project id get `account does not own the project`, with no registry content in the
error, and nothing is written. An unknown project is `unknown project: <id>`. A
project with no owner record is not usable until the existing project registration
claims it (the first account that registers the repository claims it; that is not
proof of the original owner and predates this rule). Members read only what the
owner explicitly shares (see [share-layer5](share-layer5.md)). Merge confirmation
also refuses a source memory another account owns (`unknown memory or event`),
like promotion, and it looks up an existing promotion of the source only among the
project's rows the owner owns or that have no owner, so a stored row another
account owns is never returned, aliased by the branch or updated; the owner's real
promotion is created instead. A repeat merge of an already merged branch returns the
recorded merge row only when it is such an owner-visible row of the project;
otherwise (an old branch whose recorded row is another account's, or another
project's) it fails with `project branch is already merged`, and the old record is
not repaired. The MCP tool `memory_project_freshness` is owner-only
as well. In the PostgreSQL adapter, branch and worktree registration for a project the caller does not own, or that does not exist, returns `unknown project: <id>` (a validation error, HTTP 400, the same text for both) instead of a database foreign-key error; its registry reads and maintenance still return empty results for such a project and are not changed here.

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
decisions are omitted. A file-store row that another account owns is stored but
not listed, and it cannot supersede a listed row.

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

Under the automation-first rule ([ADR 0009](../adr/0009-explicit-project-promotion.md))
merge confirmation on a project that has members is a human action, while a
project with no other members may be confirmed by an agent. Since 2026-11-14
the signal is the credential ([ADR 0008](../adr/0008-self-hosted-account-authentication.md)):
a device token (an agent) is refused with `403` and `web console session
required; device tokens cannot do this` when the project has any member besides
its owner, in both the file-store and PostgreSQL routers. A
device token may confirm a merge on a solo project, and the web session may
confirm on any owned project. Members cannot be removed, so a device token
cannot make a project solo again. In PostgreSQL the merge locks the
project row, so it cannot race a concurrent join. The member check runs after the owned-project
check, so an unknown or foreign project keeps its existing error. The other
project routes in this contract accept a device token for the owner's account
([permission table](auth-layer3.md#device-tokens-are-agent-credentials)).

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
- `promoted_from` source memory ID, returned only to the person who owns
  the source memory while it is live (it is `null` for a member and after the
  source is forgotten, expired, or superseded);
- no `source_event_ids` (empty): the copy is not tied to the personal memory's
  events;
- files, symbols, commands, and tests from the source evidence;
- redacted `promotion_reason`;
- durable retention.

The source memory remains unchanged. Branch, worktree, and commit fields are
removed from the promoted project context. Repeating the same promotion
returns the original promoted memory while that copy is live. After the copy is
forgotten or expired, promoting the personal memory again creates a new live copy
(the forgotten id stays a tombstone); branch merge confirmation follows the same
rule.

Project actions never delete personal memory. Forget and retained delete of the
promoted copy, by the project owner, hide only the copy and its shares; the
personal memory and its events stay recallable. Forgetting the personal memory
does not hide the copy. A live correction of the personal memory hides the copy
(ADR 0010), and promoting the correction creates a new copy whose `supersedes`
is the earlier copy, not the personal id. See
[ADR 0009](../adr/0009-explicit-project-promotion.md).

Secret, forgotten, expired, or repository-mismatched memories are rejected.
Project registration and promotion are person-scoped. Explicit cross-person
read of one promoted memory is defined in
[CM-CONTRACT-010](share-layer5.md).

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

`kind` and `key` are required. `kind` must be `file`, `symbol`, `decision`,
`handoff`, or `runbook`; `key` is at most 512 bytes. `content`, `sourceMemoryId`
(alias `source_memory_id`), and `commit` are optional. `content` is redacted and
at most 2048 bytes. A `sourceMemoryId` must be a memory in the project's
repository that no other account owns. `GET` returns active entities for that
project. Registering the same kind and key again returns the existing
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

Maintenance is owner-only in the file store. After authentication it requires the
caller to be the project owner: a current member and a registered
outsider who know the project id get `account does not own the project`, and the
error carries no memory id or commit. An unknown project is `unknown project:
<id>`. A project with no owner record (data from before accounts) is not
maintainable by any account until the existing project registration for that
repository claims it. The first account that registers the repository claims it,
which is not proof of the original owner; that behavior predates this rule, and
there is no other claim or repair. A member reads only what the owner
explicitly shares (see [share-layer5](share-layer5.md)); there is no
member-scoped maintenance. The PostgreSQL adapter selects the caller's own rows and
is not changed here.

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

The `cairn` binary and `cairn mcp` call the project routes above with the device token from
`cairn setup` (see [mcp-layer1](mcp-layer1.md)); a device token resolves to its account. The
commands take no account token and no store path. Each command prints the server JSON and
exits 1 with the server's message on an error:

- `cairn init [name]` (writes `.cairn/project.json`; it has no MCP tool because it writes a file)
- `cairn project-register <repository> <name> [default-branch]`
- `cairn project-list`
- `cairn promote <project-id> <memory-id> <reason>`
- `cairn project-memories <project-id>`
- `cairn project-freshness <project-id>`

MCP tools take the camelCase fields below. An unknown project is a tool error.

- `memory_project_list`
- `memory_project_register` (`repository`, `name`, optional `defaultBranch`)
- `memory_promote` (`projectId`, `memoryId`, `reason`)
- `memory_project_memories` (`projectId`)
- `memory_project_freshness` (`projectId`)

Branch merge confirmation has no CLI command or tool; on a project with members it is
web-only ([auth](auth-layer3.md)).

## Evidence

- Project init and membership by capture (2026-11-16): `init_project`, `join_context_project` in
  `crates/core/src/lib.rs` and `crates/storage/src/lib.rs`; `crates/client/src/project.rs`.
  Tests: `a_capture_with_a_project_id_makes_the_capturer_a_member_once`,
  `an_unknown_ownerless_or_own_project_id_is_ignored_by_a_capture`,
  `a_device_sync_push_with_a_project_id_joins_and_a_bare_id_keeps_session_context`,
  `init_project_creates_once_defaults_the_name_and_is_per_owner`,
  `project_people_lists_the_owner_then_members_for_owner_and_members_only` (core),
  `file_project_init_is_idempotent_and_a_capture_joins_by_project_id` (server),
  `agent_flow_on_the_file_store_server` (client), `test_project_file_at_the_git_top_level_sends_the_project_id`
  in `scripts/smoke_hook_auth.py`, and the ignored
  `postgres_a_project_id_joins_once_and_unknown_or_own_ids_are_ignored` and
  `postgres_init_project_creates_once_and_defaults_the_name` (storage).
- `docs/adr/0009-explicit-project-promotion.md`
- `docs/adr/0010-correction-keeps-linked-history.md`
- `crates/core/src/lib.rs`
- `crates/client/src/tools.rs`
- `crates/client/src/bin/cairn.rs`
- `scripts/smoke_layer4_cli.sh`
- `crates/storage/src/lib.rs`
- `migrations/0003_layer4_projects.sql`
- `migrations/0004_layer4_freshness.sql`
- `migrations/0005_layer4_project_entities.sql`
- `migrations/0006_layer4_branch_resolution.sql`
- `crates/server/src/lib.rs`
