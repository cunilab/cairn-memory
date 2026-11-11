---
id: CM-ADR-0006
title: "ADR-0006: Add branch-aware retrieval before promotion or collaboration"
status: draft
---

# ADR-0006: Add branch-aware retrieval before promotion or collaboration

- **Date:** 2026-10-26
- **Milestone:** Layer 2 — Cross-Session and Branch Continuity
- **Related:** [Roadmap](../roadmap.md), [Memory context and promotion](../specs/memory-context-and-promotion.md), [Layer 2 continuity contract](../contracts/continuity-v1.md)

## Context

Cairn already captures repository, branch, worktree, commit, and file context,
but Layer 1 recall treats every durable memory as globally searchable. That is
unsafe for coding work: an unmerged experiment from one branch can look like a
project fact when another session asks a similar question.

Layer 2 needs a useful continuity improvement without introducing PostgreSQL,
identity, synchronization, embeddings, or a graph database.

## Decision

1. Derive an explicit scope for each new durable memory: `session`, `worktree`,
   `branch`, `project`, or `global`.
2. Derive session summaries from their captured work context; explicit memories
   without repository context are user-global memories.
3. Add optional context filters to search and context retrieval across REST,
   MCP, and CLI.
4. Include source IDs, scope labels, omitted memory IDs, and a recovery hint in
   bounded context packs.
5. Preserve unfiltered Layer 1 search for local compatibility and administration.
6. Fail closed for branch/worktree memories when the required matching context
   is absent.

## Consequences

- A new session can recover project and matching branch/worktree context without
  receiving unrelated branch hypotheses.
- The file-backed store remains sufficient for the first Layer 2 slice.
- Scope is a retrieval boundary, not an authorization system; identity,
  membership, promotion, and collaboration remain later work.
- Existing exports remain version 1 because the new fields are additive and
  older records receive safe defaults during deserialization.

## Alternatives considered

### Make all session summaries global

Rejected because it leaks unmerged branch hypotheses into unrelated work.

### Require manual tags for every memory

Rejected because Cairn is AI-managed and automatic capture should remain useful
without making the person curate every observation.

### Add PostgreSQL and graph retrieval now

Rejected because Layer 2's first safety problem is scope-aware retrieval, which
can be proven with the existing deterministic file-backed adapter.
