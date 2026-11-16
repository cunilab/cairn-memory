---
id: CM-CONTRACT-009
title: "Layer 4 Project Registration and Promotion"
status: proposed
---

# Layer 4 Project Registration and Promotion

The model and trust decision are [ADR 0013](../adr/0013-shared-project-decisions-and-personal-memory-scopes.md).
This is the current contract for both file and PostgreSQL stores.

## Personal and project boundaries

Personal memory without repository context has `scope: global`; explicit
remember/lesson save without `context.repository` puts it there. A repository,
branch or worktree context gives private repository continuity. A project tag
never publishes a capture, summary or imported row. Only an explicit publication
transition sets `published_by`. Secret-marked memories cannot be published.

Recall in a checkout combines eligible global personal memory, that repository's
personal continuity and that initialized project's decisions in one relevance
ranking. Repository-only queries include private branch/worktree captures; explicit
branch/worktree filters narrow personal continuity. Published decisions
match `context.project_id`, not repository name, and carry `source: project`;
personal results carry `source: personal`. Resume/unresolved remain personal.

## Init and membership

`POST /cairn-memory/projects/init` takes
`{"repository":"host/org/repo","name":"Repo","project_id":"project-…"}`.
`name` and `project_id` are optional. With an ID it joins that existing project;
without one it explicitly creates or returns the caller's registered project for
that repository. The response has `project_id`, `repository`, `name`, `created`.
The CLI reads the existing file first, joins, and says
`already initialized, you are a member`; it never creates an additional project
for a valid existing file.

`cairn init [name]` writes and asks you to commit `.cairn/project.json`:

```json
{"project_id":"project-…","repository":"host/org/repo"}
```

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
Any account on that server with the ID can read all decisions and the email roster.
Removing the file or Git access does not remove membership; see the ADR's risk.

## Read surfaces

| Route under `/cairn-memory/` | Result |
| --- | --- |
| `GET projects` | The caller's projects and owner/member roles |
| `GET projects/{id}/memories` | One role-neutral live decision list |
| `GET projects/{id}/personal-memories` | The caller's live private repository memories |
| `GET projects/{id}/members` | Owner first, then members, with emails |
| `GET projects/{id}/conflicts` | Conflicts among the common live decision list |
| `POST projects/{id}/maintenance` | Common live decisions, with current-commit freshness |
| `GET projects/{id}/freshness` | Owner-only verification history |

Forgotten, expired, secret and superseded decisions are excluded from live lists,
recall and maintenance. Private source corrections never suppress a publication.
A live project correction suppresses its predecessor; deleting that successor may
reveal the predecessor again. No ownerless legacy project is supported.

## Promotion and preview

`POST promote` takes `projectId`, `memoryId`, `reason`, optional `dryRun: true`.
Any member may promote their own live repository memory, matching either the
canonical locator or its committed project ID after a remote rename. Other
people's memories, global preferences, secret, expired, forgotten and superseded
sources are refused.
The full copy includes content, lifecycle, evidence/context lists, next steps and
unresolved work. It clears private event/session provenance and branch/worktree
fields, retains verification evidence, records `published_by` and the actor,
and leaves the private source unchanged. Repeated live publication is idempotent.

Dry run validates the same transition and returns the full copied payload without
writing, plus `previewFingerprint`. Echo that fingerprint on promotion to refuse
a changed source with `409` and request a new preview. The console always does so.
A merge preview may supply its selected `commit`; merge accepts the same precondition. Request-generated identifiers/timestamps are allocated per request. Web
publication requires this full preview; CLI offers `promote-preview`, and MCP
`memory_promote` accepts `dryRun`. A correction's explicit later publication links
to the earlier published copy rather than to a private source ID.

## Changes and governance

The author and current owner may call `POST projects/verify` with
`projectId`, `memoryId`, `commit`, `POST correct` with `targetId`, `content`, or
`POST delete` with `id`. Other members read only. Project corrections remain
project-visible, retain original `published_by`, and record `changed_by` and
`changed_at`. Delete is retained and never cascades into private sources.
Personal source correction, expiry and forget do not change published decisions.

`POST projects/{id}/ownership` takes `{"personId":"person-…"}`. The owner's or
admin's web session may transfer to an existing member, with an audit event.
The old owner stays a member; author attribution and personal branch records stay
as they were. Owner-only project entities and verification history pass to the
new owner; private audit history does not. A device token is refused. No account/project deletion workflow exists.

## Personal branch bookkeeping

Branches and worktrees are registered/listed at `projects/{id}/branches` and
`projects/{id}/worktrees`, scoped to the caller. Identical names for two people
are independent. `branches/{branch}/close` takes `reason`; `branches/{branch}/merge`
takes `memoryId`, `commit`, `reason`. "Record merge" publishes the caller's selected
branch evidence and records only their branch/worktrees, with a preview in the
console. Devices can record merges regardless of the project's member count.
Closed branches cannot be merged; merged branches cannot be closed. Repeat merges
are idempotent for the same source/commit. Evidence entities remain available via
`projects/{id}/entities`; they do not turn private memory into a publication.

## Retired surfaces and portability

`team`, `shares`, `shares/{memory}/revoke`, `shared`, `policy`, `share-candidates`,
`share-candidates/{memory}/dismiss`, and `revocations` under a project are removed,
including CLI/MCP equivalents. The member roster replaces team spaces. The embedded
viewer has no project actions and directs users to the web console.

Decisions are read online. Personal sync is not a project decision feed, and personal
export does not back up projects, roster or governance. Import strips publication
attribution and links and brings rows in as personal memory; publication requires
an explicit new promotion. Development stores require no old-data migration.
