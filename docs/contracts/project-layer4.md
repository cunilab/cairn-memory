---
id: CM-CONTRACT-009
title: "Memory Scopes and Project Registration"
status: proposed
---

# Memory Scopes and Project Registration

This contract describes the 0.1.6 server. The design source is the
[0.1.6 foundation specification](../specs/0.1.6-foundation.md); the earlier model (publication copies,
branch merge and close, freshness, verification, similarity reviews) is retired. Pre-release databases
must be recreated: a server that finds the old `public.cairn_memories` table refuses to start.

## Memory scopes and record

| `scope` | Visible to | Written by |
| --- | --- | --- |
| `project` | Every member of the memory's project | The author, who must be a member |
| `personal` | Only the author | The author |
| `global` | Every authenticated account on the server | The author |

A memory is visible when it is not forgotten, not expired, and the caller is the owner, or it is
`global`, or it is `project`-scoped in a project the caller belongs to. Only the owner can forget it. There is
no publication or promotion step: a project memory is shared at save time by its scope.

`POST /cairn-memory/remember` takes a `SaveInput` and an optional `context` (`project_id`, `repo`,
`branch`, `worktree`, `commit`). An unknown key is a `400` naming it.

| Field | Rule |
| --- | --- |
| `kind` | `decision`, `rule`, `gotcha`, `procedure`, `fact`, `preference`, `note`. `handoff` is written only by the server |
| `title`, `summary` | Required; 1–80 and 1–240 characters |
| `reason` | Required unless `kind` is `note` |
| `fields` | Per-kind object; an unknown key is rejected. Limits: strings 1,000 characters, arrays 20 items of 300, 4 KiB in total |
| `evidence` | Up to 20 strings of at most 300 characters |
| `supersedes`, `contradicts` | One memory id each; stored as link rows. A superseded memory becomes `superseded` |
| `scope` | Optional; see below |

Per-kind `fields`: `decision` {`context`, `alternatives`, `consequences`}; `rule` {`exceptions`};
`gotcha` {`symptom`, `cause`, `fix`}; `procedure` {ordered `steps`, `when`}; `fact` {`source`};
`preference` {`example`}; `note` none. A rule's statement and a fact's statement go in `summary`.

Default scope: a decision, rule, gotcha, procedure or fact is `project` when `context.project_id` names a
project the caller belongs to, otherwise `personal`. A preference is always `personal`; asking for another scope
is a `400`. A note defaults to `personal`.

An identical save by the same owner (same kind, title, summary, reason and fields after redaction)
returns the existing id with `deduplicated: true`; evidence is merged. A save that matches a forgotten
memory is `409 forgotten`. Notes expire after 90 days and handoffs after 14. A correction is a new save with
`supersedes`; the old memory stays as history. `POST /feedback` with `helpful` or `wrong` updates usage and
can mark a memory `stale`.

`GET /cairn-memory/memories` and `GET /cairn-memory/projects/{id}/memories` return a plain list, paged with
`?limit&cursor`; the next cursor is in the `x-next-cursor` header. `GET /cairn-memory/memories/{id}` returns
one record with its entities and links. Redaction of secrets applies to every text field before hashing and storage.

## Init and membership

`POST /cairn-memory/projects/init` takes `{"repository":"host/org/repo","name":"Repo","project_id":"project-…"}`;
`name` and `project_id` are optional. With an id it joins that existing project, and a supplied `name` must
match the server's. Without an id it creates the caller's project for that repository or returns the
existing one. The response has `project_id`, `repository`, `name`, `created`. The creator is the owner and
also a member row.

`cairn init [name]` writes `.cairn/project.json` (`project_id`, `repository`, `name`; no credential, no server
address) and asks you to commit it. A valid existing file joins instead of creating. SSH and HTTPS remotes
normalize to `host/org/repo` with a lowercase host and no `.git`; a repository without a network remote uses
`local:<directory>`. A capture that carries the id joins the project idempotently; an unknown id or a failed
attachment keeps the capture personal and returns `project_attachment` with `status: personal` and a
`warning`, while success returns `status: attached`.

Possession of an id is enough to join: there is no source-control proof, invite or member removal. Any
account on that server with the id can read the project's memories and the member emails. Removing the file or
Git access does not remove membership.

## Read surfaces

| Route under `/cairn-memory/` | Result |
| --- | --- |
| `GET projects` | The caller's projects with their role |
| `GET projects/{id}/memories` | Live project memories, paged |
| `GET projects/{id}/members` | Owner first, then members, with emails |
| `GET projects/{id}/branches` | Read-only: distinct branches seen on project memories with a count and last update |
| `GET projects/{id}/conflicts` | Pairs of live project memories joined by a `contradicts` link |

## Ownership and rename

`POST projects/{id}/ownership` takes `{"personId":"person-…"}`. The owner's or an administrator's web session
may transfer it to an existing member, with an audit event; a device token is refused. The previous owner
stays a member.

`POST projects/{id}/rename` takes `{"name":"New name"}` (1–512 bytes after trimming) and returns the project.
Only the current owner may call it, with a web session or a device token; others get `401`, an invalid name `400`,
and an unchanged name is a no-op. `cairn init --rename <name>` runs it from the checkout and writes the
confirmed name to the project file, which you then commit.

## Retired surfaces

Removed in 0.1.6, with their CLI and MCP tools: `POST /projects`, `projects/verify`, branch registration, merge
and close, worktrees, project entities, freshness, maintenance, `projects/{id}/confirm`, `/correct`, `/import`,
`/sync/pull`, `/sync/diagnose`, similarity reviews, `/viewer`, and the earlier `team`, `shares`, `promote` and
`policy` routes. The console lists project memories and members and shows the derived branch and conflict
lists read-only.
