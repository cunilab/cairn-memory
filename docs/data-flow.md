---
id: CM-GUIDE-001
title: "Cairn Memory Data Flow Guide"
status: proposed
---

# Cairn Memory Data Flow Guide

This guide shows how data moves through the 0.1.6 server. Structure and tables are in
[architecture](architecture.md); exact route and field behavior is in the
[0.1.6 specification](specs/0.1.6-foundation.md) and the [contracts](contracts/project-layer4.md).

## System map

```mermaid
flowchart LR
    Agent[Agent hooks] -->|device token| API[REST API]
    CLI[cairn CLI and MCP] -->|device token| API
    Web[Web console] -->|web session| API
    API --> Store[cairn-store]
    Store --> PG[(PostgreSQL)]
    Worker[Worker] --> Store
    Repo[Committed .cairn/project.json] -->|project id| Agent
    Repo -->|project id| CLI
```

The server URL stays outside the committed file. Account creation is an admin operation; people create
device tokens in the web console. A device token can use the agent routes but not account, device or
ownership routes. A password reset revokes web sessions, not device tokens.

## Capture and distill

```mermaid
flowchart TD
    Hook[Hook event] --> Redact[Redact secrets and work context]
    Redact --> Post[POST observe]
    Post --> Obs[(observations under a session)]
    Post --> Attach{project id in the request and known?}
    Attach -->|yes| Join[Join the project, idempotently]
    Attach -->|no| Personal[Stay personal]
    Obs --> End[Session end or checkpoint]
    End --> Job[distill job]
    Job --> Handoff[one handoff per repo, branch and worktree]
    Job --> Notes[typed notes, fail-then-fix gotcha, test command fact]
```

Capture is always personal. A failed project attachment never fails the capture; the reply carries
`project_attachment.status` (`attached` or `personal`). Event ids are stable: an identical replay is
acknowledged and a changed replay is a conflict. The hook keeps a bounded outbox for outages. Observations
are raw evidence and are never recalled directly; they expire and are swept, and are deleted a week after their
session is distilled. At session start the hook also answers the brief's `check` (which cited files and symbols still
exist on the default branch, which branches are merged) with `POST /session/repo-state`, which marks memories stale or
active again and moves merged-branch memories to the default branch.

## Save

```mermaid
flowchart TD
    Input[SaveInput from an agent] --> Prep[Redact, validate fields for the kind, kind policy]
    Prep --> Dup{Same identity hash?}
    Dup -->|forgotten| Reject[409 forgotten]
    Dup -->|exists| Same[Return the existing id, deduplicated]
    Dup -->|new| Write[Write memory, entities, links]
    Write --> Link[link job]
    Link -->|shared file, module, symbol, test; gotcha fix/cause| Auto[auto links, same project or owner, at most 8]
    Write --> Embed[embed job]
    Embed --> Consolidate[consolidate job]
    Consolidate -->|project rule or gotcha with a twin in another project, cosine 0.92 or more| Promote[One global copy, redacted, linked to both sources]
    Promote -->|fewer than 2 live sources| Withdraw[Copy expires]
    Consolidate -->|cosine 0.92 or more, same owner, scope, project, kind| Super[Older memory superseded]
    Consolidate -->|0.75 to 0.92| Relates[relates link]
```

The default scope of a decision, rule, gotcha, procedure or fact is `project` in an initialized
repository and `personal` otherwise; a preference is always `personal`. A correction is a new save with
`supersedes`; the old memory stays as history. A quarantined job is retried once at the first start after a server
upgrade, then by the sweep a day later, with one attempt left, so nobody requeues jobs by hand.

## Recall

```mermaid
flowchart LR
    Query[Query and work context] --> Vis[Visible rows: owner, project member, global]
    Vis --> Lex[Lexical]
    Vis --> Ent[Entity]
    Vis --> Vec[Vector]
    Lex --> Fuse[RRF fusion]
    Ent --> Fuse
    Vec --> Fuse
    Fuse --> Gate[Relevance gate: entity, coverage, cosine]
    Gate --> Rank[Capped adjustments, plus or minus 30 percent]
    Rank --> Pack[Two per file, token budget, limit, more: n]
```

One ranking serves personal, project and global memories; the visibility rule runs in the same query before
any id leaves the store (for the vector channel, inside the HNSW index scan). `kind=handoff` on `/search` finds the caller's own handoffs.
The session brief is this branch's last handoff, the project's rules, decisions and gotchas ranked by the
session's recent files, branch and feedback, and two personal preferences, at most 1,200 bytes.

## Projects and console

`cairn init` creates the project (the caller becomes the owner) and writes `.cairn/project.json`; commit it.
**Possessing the id is enough to join:** anyone with an account on the server and a copied id can read the
project's memories and the member emails. There are no invites and no member removal. The owner can rename
the project and transfer ownership; the previous owner stays a member.

The console shows memories, a graph of memories and links, projects with their members, derived branch
lists and conflicts (read-only), background jobs, devices and accounts. Forgetting is the only delete:
it erases the text and keeps the identity hash so the same memory cannot silently return.
