---
id: CM-ADR-0014
title: "Memory scopes, distillation and hybrid recall"
status: proposed
---

# Memory scopes, distillation and hybrid recall

Date: 2026-11-17.

## Context

Fact research of two open memory systems, akitaonrails/ai-memory and mempalace,
showed practices Cairn lacks: automatic distillation of sessions into typed
notes, hybrid ranking, a bounded session-start brief, usage-based retention and
capture hygiene. Cairn's current split into project, personal-per-repo and
personal-global memory, plus an explicit promote step, makes members do manual
work that the product direction (minimal human interaction) rejects.

This is a proposal. When accepted it supersedes the scope, personal-memory and
promotion parts of [ADR 0013](./0013-shared-project-decisions-and-personal-memory-scopes.md)
(decisions 1, 2 and 5 there, and the `source: personal | project` labelling in
decision 7). The rest of ADR 0013 remains. Pre-release rules still apply: no
data migration or legacy repair. v0.1.4 uses no LLM and adds no benchmark.

Already implemented in the v0.1.4 branch and unchanged by this ADR:
`.cairn/project.json` carries the project name, `cairn init --help` is resolved
locally, and only the owner can rename (`cairn init --rename <name>`). See the
2026-11-17 amendment in ADR 0013.

## Decision

The owner decided the following.

0. **Three memory scopes** replace project, personal-per-repo and
   personal-global:
   - *Project memory*: rules, decisions, gotchas, procedures, facts and raw
     captures (prompts, tool results, session summaries) from repositories
     initialized with `cairn init`. Visible to all project members.
   - *Personal behaviour memory*: how the agent should answer the person,
     preferences and habits. Private to that person and never shared.
   - *Global memory*: design patterns and important lessons, with the same note
     types. Visible to everyone on the server. Default edit and delete rights:
     author and administrator.

   The agent chooses the scope explicitly when saving, through an MCP/CLI
   parameter. Captures in an initialized repository go to project memory. The
   old personal per-repo scope and the promote step are removed.
   Consequence: raw captures are visible to all project members and global
   notes to all server users. Redaction still applies before storage.
1. **Rule-based distillation at session end**, without an LLM: a session note,
   a handoff and typed notes (decision, gotcha, procedure, rule, fact), each
   linked to its source captures. In an initialized repository they go
   straight into project memory.
2. **Hybrid recall** combining full-text, entity, links and local vector
   embeddings. Embeddings come from a small local model (such as MiniLM) run
   inside `cairn-server`; no external API is called. Rankings are fused by
   reciprocal-rank fusion, `1 / (60 + rank + 1)`, with type weights
   (rule/decision above notes above raw captures) and a pinned boost. Results
   are labelled project, personal or global.
3. **Session-start brief** injected by the hook, bounded in size and fenced as
   data, not instructions: personal behaviour memory, the last handoff, top
   project rules and decisions, and top global notes.
4. **Retention score** with reinforcement on use. A memory below the threshold
   is ranked lower only and is never auto-deleted. Rules and decisions do not
   decay.
5. **Capture hygiene**: assistant text is not captured by default (opt-in); a
   marker file skips capture per repository or folder; secrets are scrubbed
   before truncation.
6. **Feedback** helpful/stale on any memory. Any member's feedback on project
   memory changes ranking for everyone. For global memory any user may give
   feedback and it affects everyone.

## Open: memory record shape

The fields of a memory (title, description, reason and others) are being
decided by the owner and will be added to this section. Until then no storage
format or contract change follows from this ADR.

## Trust and limits

- Project membership is by project ID (ADR 0013), so raw captures, not only
  published decisions, are readable by anyone who joins. Redaction is the
  only barrier; it is best-effort.
- Any server user can read global notes and, through feedback, change their
  ranking. Edit and delete stay with author and administrator.
- Rule-based distillation is heuristic and can miss or mistype notes. Local
  embeddings add a model download and CPU cost to the server.
- Nothing here is implemented or measured yet; claims about recall quality
  need recorded evidence before acceptance.

## Implementation checklist

- [ ] 0. Three scopes: scope parameter in MCP/CLI, per-repo personal scope and promote removed, captures in initialized repos go to project memory.
- [ ] 1. Rule-based session-end distillation: session note, handoff, typed notes linked to captures.
- [ ] 2. Hybrid recall: full-text, entity, links, local embeddings, RRF, type weights, pinned boost, scope labels.
- [ ] 3. Bounded, data-fenced session-start brief from the hook.
- [ ] 4. Retention score with reinforcement; rules/decisions exempt; no auto-delete.
- [ ] 5. Capture hygiene: assistant text opt-in, skip marker file, scrub before truncation.
- [ ] 6. Helpful/stale feedback affecting shared ranking.

## Evidence

Source research: fact notes on akitaonrails/ai-memory and mempalace (decay with
access reinforcement, RRF ranking, sanitization hooks, opt-in assistant
capture). Related: [ADR 0013](./0013-shared-project-decisions-and-personal-memory-scopes.md),
[ADR 0010](./0010-correction-keeps-linked-history.md).
