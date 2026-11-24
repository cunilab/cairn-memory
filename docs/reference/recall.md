---
id: CM-REF-004
title: "Recall and background processing"
status: draft
---

# Recall and background processing

## Hybrid recall

Search applies visibility and context filters, then combines lexical, entity, vector and link evidence. Vector search uses the local 384-dimensional MiniLM model when configured; lexical, entity and link search continue without it. Reciprocal-rank fusion combines candidate channels. The relevance gate accepts an entity match, cosine at least 0.43, or at least half the query words with cosine at least 0.35 when an embedding is available. Without an embedding, coverage alone can pass. Results are ranked and packed under a limit and token budget; `more` reports omitted results. A relevant paraphrase can miss the gate.

Project scope requires membership. Global memories match every context; a personal handoff also follows the repository, branch and worktree context. Candidate notes rank lower and are excluded from briefs until used or confirmed. Proof counts add a bounded ranking boost. Stale and superseded memories rank lower and carry a label. Forgotten memories are excluded. Aliases may be stored but are not currently used in vector search. See [the benchmark](../../bench/README.md) for the gate's limits.

## Session handoffs and distillation

The hook starts a brief of at most 1,200 bytes containing a relevant handoff, project memories and personal preferences. Checkpoint and session-end processing turns typed captures into notes, a digest and, when appropriate, a handoff. Explicit `next:` or `todo:` lines supply next steps. Short chats and automated or subagent sessions do not overwrite a useful handoff. Raw observations are not returned by memory search.

## Durable jobs

The worker runs distill, embed, link and consolidate jobs. Jobs use leases, retry with backoff and quarantine on repeated failure. The console shows quarantined jobs; a sweep and a server-version change can retry them. Finished jobs and raw observations have retention limits, while memories do not expire by age.

Consolidation can link related records or supersede a near duplicate. Differences in numbers, versions or file names veto an automatic merge. Explicit corrections use `supersedes`; declared contradictions are kept as links. Search cards give compact results; `memory_get` returns full records and links.
