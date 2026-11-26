---
id: CM-DOC-TECHNICAL-PROCESSING
title: "Processing and jobs"
status: accepted
description: "Follow typed captures through checkpoints, distillation, retrieval indexing and retries."
---

# Processing and jobs

## Session handoffs and distillation

Checkpoint and session-end processing turns typed captures into notes, a digest and, when appropriate, a handoff. A session that mixes projects, or project and personal work, distills each attachment on its own; a mixed or indeterminate cohort stays personal. The digest's test line is the last execution of that command. Explicit `next:` or `todo:` lines supply next steps. Short chats and automated or subagent sessions do not overwrite a useful handoff. Raw observations are not returned by memory search.

## Durable jobs

The worker runs distill, embed, link and consolidate jobs. Jobs use leases, retry with backoff and quarantine on repeated failure. The console shows quarantined jobs; a sweep and a server-version change can retry them. Finished jobs and raw observations have retention limits, while memories do not expire by age.

Consolidation can link related records or supersede a near duplicate. Differences in numbers, versions or file names veto an automatic merge. Explicit corrections use `supersedes`; declared contradictions are kept as links. Search cards give compact results; `memory_get` returns full records and links.

The `distill` job extracts records, a digest and eligible handoffs from typed observations. The `embed` job populates the vector index when local embeddings are available. The `link` job derives evidence from shared entities. The `consolidate` job applies the similarity and merge guardrails to active memories. See the [data model](data-model.mdx) for the owner of each durable record and [the architecture](index.mdx#main-flows) for the capture-to-recall path.
