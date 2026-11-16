---
id: CM-ADR-0009
title: "Explicit evidence-backed project promotion"
status: proposed
---

# ADR 0009 — Explicit evidence-backed project promotion

## Context

Cairn already stores branch-aware personal memories with repository, branch,
worktree, files, symbols, commands, tests, and lifecycle metadata. A project
memory must not appear merely because a branch memory is relevant. Promotion
needs an explicit owner action, a registered project boundary, and a traceable
source.

## Decision

1. A project is registered per person and repository in the first Layer 4
   slice. Project identity is represented by a stable project ID and a
   repository locator.
2. `POST /cairn-memory/promote` is the explicit promotion transition. It
   creates a project-scoped decision memory rather than mutating the source.
3. The promoted memory preserves source event IDs, source memory ID, files,
   symbols, commands, tests, lifecycle dimensions, and a redacted promotion
   reason. Branch, worktree, and commit fields are cleared from the promoted
   project context so branch hypotheses do not masquerade as project facts.
4. Secret memories, forgotten memories, expired memories, and repository
   mismatches fail closed. Repeating the same promotion is idempotent.
5. PostgreSQL records the project registry, promotion provenance, and an audit
   event. The file-backed adapter exercises the same owner-facing flow for
   local compatibility.
6. Project decisions retain the source verification commit. A later explicit
   project verification updates that commit and freshness; retrieval against a
   different commit reports stale evidence instead of silently treating it as
   current.
7. Person-owned project branches and worktrees are registered separately from
   promoted memory. Their composite project/person scope is checked on every
   operation, and project-memory inspection is a read-only bounded view.
8. Merge confirmation is an explicit owner action at
   `POST /cairn-memory/projects/{projectId}/branches/{branchId}/merge`. It
   requires source memory evidence from that branch, records the merge commit
   as the project decision's verification commit, transitions the branch to
   `merged`, and resolves attached worktrees. Abandoned branches use an
   explicit `close` transition without promotion; both transitions are
   idempotent or fail closed on incompatible state.

## Amendment 2026-11-14: promotion never ties the copy to personal events

Reason: project actions must never delete a person's personal memory.

Superseding the source-event part of decision 3:

1. The promoted copy keeps `promoted_from` (the personal memory id) and clears
   `source_event_ids`. It no longer shares the personal memory's source events,
   so forget, retained delete, and source cascade on the copy stop at the copy
   and its shares. They never tombstone the personal memory or its events.
2. Forgetting the personal memory does not hide the copy, which was
   deliberately shared. `promoted_from` then stays stored but inert: it is not
   followed and not exposed. It is also inert while the personal memory is
   expired or superseded.
3. `promoted_from` is returned in recall and project listing only to the
   person who owns the personal memory, and only while that memory is live. A
   shared read for a member omits it and `source_memory_id`.
4. A live correction of the personal memory hides its promoted copies like the
   original (ADR 0010). Promoting the correction links the new copy to the
   earlier copy with `supersedes`, never to the personal id.

Old promoted rows are not migrated and keep their stored source events; a forget
of such a row no longer cascades to them.

## Amendment 2026-11-14: automation first

Reason: routine memory upkeep must not wait for a person. Cairn is AI-managed and
person-owned; a human acts only where trust, ownership, or irreversibility is at
stake.

1. Routine steps are automatic: closing idle or superseded sessions and deriving
   their summaries, registering the project and branch that a capture names,
   and ordering contradicting recall (newer first, nothing auto-resolved). The
   rules are in [CM-CONTRACT-006](../contracts/continuity-v1.md) and
   [CM-CONTRACT-009](../contracts/project-layer4.md).
2. A human acts for: accounts and sign-in, devices, share and revoke,
   share-policy changes, delete and forget, branch close, and merge
   confirmation on a project that has members.
3. Automatic registration never claims a project with no owner record, uses a
   project the person owns or is a member of before creating one, and creates it
   owned by the capturing person.
4. Status: this amendment is `proposed`. Decision 2 for merge is enforced since
   2026-11-14 through the credential kind chosen in
   [ADR 0008](0008-self-hosted-account-authentication.md): a device token is
   refused (`403`) on a project with any member besides its owner, and the web
   session is the human surface. The other decision 2 actions (share
   and revoke, share policy, delete and forget, branch close) accept a device
   token for the owner's account; accounts, sign-in and devices are web-session
   only. That follows the owner's choice in ADR 0008; this ADR's wording for
   those actions is not enforced as human-only.

## Consequences

- Project search and context can retrieve explicitly promoted decisions through
  the existing repository-scoped filters without requiring a team space.
- Stale/superseded maintenance and collaboration permissions can build on the
  immutable source/promoted relationship and the recorded branch resolution.
- Project registration currently uses the existing self-hosted account
  boundary; cross-person project membership remains Layer 5.

## Evidence

- `crates/core/src/projects.rs`
- `crates/storage/src/lib.rs`
- `migrations/0003_layer4_projects.sql`
- `migrations/0004_layer4_freshness.sql`
- `migrations/0005_layer4_project_entities.sql`
- `migrations/0006_layer4_branch_resolution.sql`
- `crates/server/src/lib.rs`
- `crates/storage/tests/postgres.rs`
