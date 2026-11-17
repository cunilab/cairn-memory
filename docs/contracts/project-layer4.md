---
id: CM-CONTRACT-009
title: "Memory Scopes and Project Registration"
status: proposed
---

# Memory Scopes and Project Registration

The current scope and record contract is [ADR 0014](../adr/0014-memory-scopes-distillation-and-hybrid-recall.md), items 0 and 7. Project identity and governance retain [ADR 0013](../adr/0013-shared-project-decisions-and-personal-memory-scopes.md).
Both file and PostgreSQL adapters implement this contract. Pre-release stores must be reset; no legacy data compatibility is supported.

## Memory scopes and record

| `scope` | Visibility | Mutation |
| --- | --- | --- |
| `project` | All members of `context.project_id`, including raw captures and summaries | Author or current project owner |
| `personal` | Only the author; personal behaviour, habits and preferences | Author |
| `global` | Every authenticated user on this server | Author or administrator |

`personal` means personal behaviour, never personal-per-repository memory.
Agents explicitly supply scope on `POST /cairn-memory/remember` (`memory_save`, CLI `save` or `lesson`).
A project save requires membership and `context.project_id`. Personal and global notes follow the person/server regardless of repository context; `context` retains provenance and optional `applies_to` records applicability.
Captures carrying a valid initialized project ID are stored as project records (`kind: capture`), with summaries also project-visible. There is no publication or promotion step.
Redaction applies before storage to common fields, typed fields, context and captures. Shared captures are readable after redaction even when classified sensitive or secret; redaction is best effort.

Required on explicit save: nonblank `title`, `summary`, `reason`, plus `scope`.
Missing fields return a validation error naming the field. Fields are flat snake_case:

- Common: `title`, `summary`, optional `details`, `reason`, `applies_to`, `evidence`, `tags`, `entities`, `status` (`active`, `stale`, `superseded`), `supersedes`, `pinned`, `importance` (0–100; also reflected in the existing lifecycle object), server-assigned `author`, `created_at`, `changed_by`, `changed_at`.
- `decision`: `decision_context`, `alternatives`, `consequences`. `context` remains the structured work context; `decision_context` is the decision's rationale context.
- `rule`: `rule`, `exceptions`.
- `gotcha`: `symptom`, `cause`, `fix`.
- `procedure`: ordered `steps`, `when`.
- `fact`: `statement`, `source`.
- `preference` (personal only): `preference`, `example`.

`alternatives`, `steps`, `applies_to`, `evidence`, `tags`, `entities` are string arrays. Other typed fields are optional strings. `kind` defaults to `explicit`; all typed fields are optional. Capture/session records may omit agent-authored rationale. `content` remains a derived lexical body including title, summary, details and reason. Title matches have the highest lexical weight in both stores. Recall stays lexical in this stage, with `source: project | personal | global`; no embeddings or distillation are added.

```json
{"scope":"project","kind":"decision","title":"Serialize writes","summary":"Use a transaction for memory updates.","reason":"Prevent concurrent updates from losing data.","context":{"project_id":"project-…"},"decision_context":"Multiple agents write concurrently","alternatives":["independent writes"],"consequences":"short lock duration","evidence":["cargo test"],"pinned":true}
```

`GET /cairn-memory/memories` lists all live records the caller may read; `GET /cairn-memory/memories/{id}` returns every field of a readable record. The console Memories page shows title, summary and scope; its detail sheet shows the full record. Project lists use the same record shape.

## Init and membership

`POST /cairn-memory/projects/init` takes
`{"repository":"host/org/repo","name":"Repo","project_id":"project-…"}`.
`name` and `project_id` are optional. With an ID it joins that existing project;
if `name` is supplied it must match the server name (after trimming), checked
before membership changes. Without an ID it explicitly creates or returns the
caller's registered project for that repository; a supplied different name is
refused instead of silently ignored. The response has `project_id`, `repository`, `name`, `created`.
The CLI reads the existing file first, joins, and says
`already initialized, you are a member`; it never creates an additional project
for a valid existing file.

`cairn init [name]` writes and asks you to commit `.cairn/project.json`:

```json
{
  "project_id": "project-…",
  "repository": "host/org/repo",
  "name": "Repo"
}
```

Fresh `cairn init <name>` creates that name; omission defaults to the repository's
short name. A normal re-init joins without renaming. A different positional name
from the file is refused locally, before a request. A file/server name mismatch
is refused by the server. CLI and hook readers accept older files without `name`;
init fills it from the server after a successful join. Capture uses only the ID;
the display name is not an identity or an authorization field.

`cairn init --help`, `-h`, `cairn --help`, and `cairn help init` print usage and
exit 0 before configuration, stdin, network calls or update checks. Every known
subcommand supports `--help` and `-h` as its sole argument. Other arguments
starting with `-` are rejected before requests; the only action options are
`update --check` and `init --rename <name>`. There is no `--` escape for flag-like
positional values.

The file contains no credential or server address. SSH and HTTPS remotes normalize
to `host/org/repo`, host lowercase, without `.git`. A repository without a network
remote uses `local:<directory>`. UI names are short, while the full locator remains
available. The configured server must be the one that knows the committed ID.

A capture with the ID joins idempotently and can register the person's branch.
Capture without an ID creates no project. Unknown IDs and attachment failures
keep the capture personal and return `project_attachment` with `status: personal`
and a `warning`; successful attachment returns `status: attached` and no warning.
The next capture or duplicate replay retries attachment. Broken/unreadable files
warn on CLI/hook stderr. Init refuses a broken file instead of overwriting it.

Possession of an ID is enough to join: no SCM proof, invite or member removal.
Any account on that server with the ID can read all project records and the email roster.
Removing the file or Git access does not remove membership; see the ADR's risk.

## Read surfaces

| Route under `/cairn-memory/` | Result |
| --- | --- |
| `GET memories`, `GET memories/{id}` | All readable live records, or one full record |
| `GET projects` | The caller's projects and owner/member roles |
| `GET projects/{id}/memories` | All live project notes, captures and summaries |
| `GET projects/{id}/members` | Owner first, then members, with emails |
| `GET projects/{id}/conflicts` | Conflicts among live project memories |
| `POST projects/{id}/maintenance` | Live memories with current-commit freshness |
| `GET projects/{id}/freshness` | Owner-only verification history |

Forgotten, expired and superseded records are excluded from normal reads.
A live successor hides its predecessor within its scope and project; deletion of the successor can reveal the predecessor.

## Changes and governance

The author and current owner may call `POST projects/verify` with
`projectId`, `memoryId`, `commit`, `POST correct` with `targetId`, `content`, or
`POST delete` with `id`. Other members read only. Project corrections remain project-visible and retain their author. Global corrections keep global scope and require the author or administrator. Delete is retained.
Declaring a contradiction, dismissing a similarity review, and explicit supersede use this same mutation rule. The ranking factor, review visibility, and routes are in [the continuity contract](continuity-v1.md#declared-contradictions-v014).
Corrections retain typed fields and author attribution, and record the actor.

`POST projects/{id}/ownership` takes `{"personId":"person-…"}`. The owner's or
admin's web session may transfer to an existing member, with an audit event.
The old owner stays a member; author attribution and personal branch records stay
as they were. Owner-only project entities and verification history pass to the
new owner; private audit history does not. A device token is refused. No account/project deletion workflow exists.

## Project rename

`POST projects/{id}/rename` takes `{"name":"New name"}` and returns the updated
project. Only the current owner may call it, with their web session or device
token. Members, non-members and an administrator who is not the owner are refused
with `401` (the existing ownership-error mapping), as are unauthenticated
requests. Invalid names are `400`. Names are trimmed and must contain 1–512 bytes. An unchanged name is a
no-op. A change updates `updated_at` and records `project_renamed` with the project
ID, `previous_name` and `name` in the owner's private audit, atomically in both
stores. PostgreSQL locks the project row so an ownership transfer cannot race
the authorization check. ID, repository, membership and decisions are preserved.

The CLI command is `cairn init --rename "New name"`, run inside the initialized
checkout with the owner's device configuration. It targets the committed project
ID and writes the server-confirmed name back to the pretty JSON file. It works
with an old file that lacks `name`, or one whose name differs from the server
(e.g. a project accidentally named `--help` before 0.1.4). Commit and distribute
the updated file. Ordinary members with stale files should pull that commit;
init does not silently rename or overwrite a conflicting name. A REST-only rename
requires updating committed files too. If the server rename succeeds but the local
write fails, the error is reported and the old file is retained; retry the same
command (the server's unchanged-name path adds no duplicate audit).

## Personal branch bookkeeping

Branches and worktrees are registered/listed at `projects/{id}/branches` and
`projects/{id}/worktrees`, scoped to the caller. Identical names for two people
are independent. `branches/{branch}/close` takes `reason`; `branches/{branch}/merge`
takes `memoryId`, `commit`, `reason`. "Record merge" references and verifies the caller's existing project-scoped branch evidence and records only their branch/worktrees. It creates no copy. Devices can record merges regardless of the project's member count.
Closed branches cannot be merged; merged branches cannot be closed. Repeat merges
are idempotent for the same source/commit. Evidence entities remain available via
`projects/{id}/entities`; they do not turn private memory into a publication.

## Retired surfaces and portability

`team`, `shares`, `shares/{memory}/revoke`, `shared`, `policy`, `share-candidates`,
`share-candidates/{memory}/dismiss`, and `revocations` under a project are removed,
including CLI/MCP equivalents. `promote`, `promote-preview`, `memory_promote` and `projects/{id}/personal-memories` are also removed (HTTP 404/405); they have no replacement publication step. The member roster replaces team spaces. The embedded
viewer has no project actions and directs users to the web console.

Decisions are read online. Personal sync is not a project decision feed, and personal
export does not back up projects, roster or governance. Import strips project association and brings rows in as personal memory. Development stores require no old-data migration.
