---
id: CM-ADR-0009
title: "Explicit evidence-backed project promotion"
status: accepted
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

## Consequences

- Project search and context can retrieve explicitly promoted decisions through
  the existing repository-scoped filters without requiring a team space.
- Stale/superseded maintenance and collaboration permissions can build on the
  immutable source/promoted relationship and the recorded branch resolution.
- Project registration currently uses the existing self-hosted account
  boundary; cross-person project membership remains Layer 5.

## Evidence

- `crates/core/src/lib.rs`
- `crates/storage/src/lib.rs`
- `migrations/0003_layer4_projects.sql`
- `migrations/0004_layer4_freshness.sql`
- `migrations/0005_layer4_project_entities.sql`
- `migrations/0006_layer4_branch_resolution.sql`
- `crates/server/src/lib.rs`
- `crates/storage/tests/postgres.rs`
