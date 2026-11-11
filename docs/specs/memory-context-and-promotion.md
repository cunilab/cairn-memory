---
id: CM-SPEC-002
title: "Specification — Memory Context and Promotion"
status: draft
---

# Specification — Memory Context and Promotion

| Field | Value |
| --- | --- |
| Status | Draft |
| Owner | Cairn Memory maintainers |
| Target | Layers 1–6: personal memory, branch-aware context, promotion, and advanced retrieval |
| Related | [Product](../product.md), [Roadmap](../roadmap.md), [Architecture](../architecture.md), [Graph-assisted retrieval](./graph-assisted-retrieval.md) |

## Summary

Cairn Memory should preserve the useful context behavior already present in AgentMemory while making it person-owned, automatically managed, server-authoritative when synchronized, and branch-aware. A person should not need to decide what to save or manually assemble context. Cairn should not inject every historical record into every prompt; it should assemble a bounded context pack from explicit memory scopes, current work context, relevant files and symbols, recent session state, lessons, pinned guidance, and unresolved work.

Promotion is a controlled escalation process. A session observation may be retained automatically, but it does not automatically become a global fact. Branch-specific work should remain branch-specific until it is merged, independently confirmed, or explicitly promoted.

## Memory autopilot

The default lifecycle is automatic:

```text
observe -> redact -> classify -> retain/compress/expire -> derive evidence-backed memory -> retrieve when relevant
```

The autopilot may decide what is useful, stale, redundant, or worth summarizing. It must preserve source IDs and scope, keep uncertain or sensitive material private, and surface exceptions rather than forcing the person to curate every item. The person remains the owner and can correct, forget, export, or override the result.

## Context hierarchy

| Scope | Meaning | Default visibility |
| --- | --- | --- |
| Global user | Durable user preference or personal working style | User and authorized agents |
| Organization/team | Shared institutional or team practice | Members of the relevant space |
| Project | Repository/product architecture, conventions, and durable decisions | Project members and authorized agents |
| Branch/worktree | Unmerged implementation hypothesis, experiment, or branch-specific decision | Actors with access to that branch/worktree |
| Session | Current agent interaction, raw observations, and temporary hypotheses | Session actor and authorized operators |
| File/symbol/entity | Structural context for a file, symbol, service, issue, or external system | Inherited from source scopes |

Retrieval should search the current scope first, then explicitly allowed ancestors. A branch-local memory must not silently appear as a project-wide fact merely because it is semantically similar.

## Work context

Every agent session should capture, when available:

- organization, project, repository, and canonical repository ID;
- branch name and branch ID;
- worktree ID/path or remote workspace ID;
- base branch and base commit;
- current commit SHA;
- pull request number and status;
- actor, agent, device, and MCP token identity;
- files, symbols, tests, commands, and services touched.

This context is used for retrieval filters, promotion decisions, conflict detection, and handoff generation.

## Promotion lifecycle

```text
session observation
  -> classified evidence
  -> branch/worktree memory candidate
  -> project candidate
  -> team/organization candidate
  -> user-global preference candidate
```

Promotion must preserve source IDs, scope, actor, branch, commit, confidence, sensitivity, and the reason for promotion.

### Promotion rules

- A session observation may become branch/worktree memory automatically when it is important and well-supported.
- Branch/worktree memory may become project memory when the branch is merged, the fact is independently confirmed, or a user explicitly promotes it.
- Project memory may become team or organization memory only through explicit policy or approval.
- A project-specific fact must not become global user memory.
- A user preference may become global user memory when the user confirms it or repeated evidence supports it across projects.
- Sensitive material cannot be promoted automatically to a broader scope.
- Contradictions reduce promotion confidence and create a conflict rather than selecting a winner silently.

## Scores

Do not use one score for every decision. Store independent dimensions:

```text
importance     how much the fact matters
confidence     how strongly the evidence supports it
freshness      how recently it was verified
authority      trust of the source actor or system
utility        how often it helps retrieval or work
sensitivity    risk if exposed
shareability   whether policy allows broader visibility
```

Derived values may include:

```text
retrieval_score
promotion_score
staleness_score
conflict_score
```

Authorization is never a score. A high-scoring memory cannot bypass permissions.

## Context pack

The server should assemble a bounded context pack in this order:

1. Current user/agent identity and permissions.
2. Current repository, branch, worktree, commit, and active project.
3. Pinned project guidance and user preferences.
4. Open tasks, unresolved questions, and recent handoff state.
5. Relevant project memories and verified decisions.
6. Relevant branch/worktree memories.
7. File and symbol history for files currently touched.
8. Recent session summary and high-importance observations.
9. Lessons and recurring failure patterns.
10. Graph neighbors needed to explain files, symbols, services, decisions, or dependencies.

The pack must include source IDs and scope labels, fit a token budget, and provide a retrieval path for omitted material.

## AgentMemory features to carry forward

The upstream foundation already provides useful patterns to preserve:

- pinned global and project slots for compact always-available guidance;
- project profiles with concepts, files, conventions, and common errors;
- session summaries with decisions and modified files;
- high-importance observation fallback when a summary is unavailable;
- lessons with confidence, reinforcement, decay, project scope, and source IDs;
- working memory with pinned entries, importance, recency, access count, and paging;
- sliding-window enrichment that resolves pronouns and adds context bridges;
- query expansion with reformulations, temporal concretizations, and entity extraction;
- smart search with compact results and explicit expansion IDs;
- file-context lookup across previous sessions;
- graph-assisted retrieval and authorization-aware project/agent filtering.

These should become server-side services with explicit organization, project, branch, worktree, and session filters rather than remaining process-local state.

## Acceptance criteria

- [ ] A person can use the memory lifecycle without manually saving, tagging, filing, or summarizing every useful observation.
- [ ] An agent starting on a branch receives relevant project context without receiving unrelated branch-local hypotheses.
- [ ] A memory created on one branch is not promoted to project scope without merge, confirmation, or explicit approval.
- [ ] Context packs contain source IDs, scopes, branch/worktree metadata, and omitted-result recovery hints.
- [ ] A user preference can be available across projects while a project convention remains project-scoped.
- [ ] A private memory cannot enter a shared context pack through a summary, embedding, graph edge, or file-context result.
- [ ] Context assembly works without an LLM key; LLM summaries and query expansion improve it asynchronously.
- [ ] The viewer explains why an item was included, its scope, its evidence, its score dimensions, and how to correct, forget, promote, or demote it.
