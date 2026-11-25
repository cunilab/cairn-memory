---
id: CM-REF-004
title: "Recall and background processing"
status: draft
---

# Recall and background processing

## Hybrid recall

Search applies visibility and context filters, then combines lexical, entity, vector and link evidence. Vector search uses the local 384-dimensional MiniLM model when configured; lexical, entity and link search continue without it. Reciprocal-rank fusion combines candidate channels. The relevance gate accepts an entity match, cosine at least 0.43, or at least half the query words with cosine at least 0.35 when an embedding is available. Without an embedding, coverage alone can pass. Results are ranked and packed under a limit and token budget; `more` reports omitted results. A relevant paraphrase can miss the gate.

Project scope requires membership. Global memories match every context; a personal handoff also follows the repository, branch and worktree context. Candidate notes rank lower and stay out of briefs. A helpful vote can confirm a coached agent note, and a clean correction saves at normal rank. System notes can also be confirmed through use. Proof counts add a bounded ranking boost. Stale and superseded memories rank lower and carry a label. Forgotten memories are excluded. Aliases may be stored but are not currently used in vector search. While that gate is off, new alias vectors are not generated; existing alias rows and vectors are kept. See [the benchmark](../../bench/README.md) for the gate's limits.

## Automatic recall

The session brief includes a relevant handoff, project memories and personal preferences within 1,200 bytes. File topics match the exact file, its module or symbol names from a bounded sample of tool input. Results rank rules and gotchas first, followed by decisions, then facts and procedures. Within a group, helpful votes take precedence over recency. Each pushed memory includes a short reason for showing it.

Only memories with normal or proven trust enter automatic recall. The server shares an eight-memory budget across briefs and file topics for each person and session ID. A memory can use only one slot in that session. The idle sweep removes push records after seven days. Explicit searches and gets have their own limits.

## Session handoffs and distillation

Checkpoint and session-end processing turns typed captures into notes, a digest and, when appropriate, a handoff. A session that mixes projects, or project and personal work, distills each attachment on its own; a mixed or indeterminate cohort stays personal. The digest's test line is the last execution of that command. Explicit `next:` or `todo:` lines supply next steps. Short chats and automated or subagent sessions do not overwrite a useful handoff. Raw observations are not returned by memory search.

## Durable jobs

The worker runs distill, embed, link and consolidate jobs. Jobs use leases, retry with backoff and quarantine on repeated failure. The console shows quarantined jobs; a sweep and a server-version change can retry them. Finished jobs and raw observations have retention limits, while memories do not expire by age.

Consolidation can link related records or supersede a near duplicate. Differences in numbers, versions or file names veto an automatic merge. Explicit corrections use `supersedes`; declared contradictions are kept as links. Search cards give compact results; `memory_get` returns full records and links.
