---
id: CM-DOC-TECHNICAL-PROCESSING
title: "Processing and jobs"
status: accepted
description: "Follow typed captures through checkpoints, distillation, retrieval indexing and retries."
---

# Processing and jobs

## Session handoffs and distillation

[Distillation](../glossary.md#distill) turns captured [observations](../glossary.md#observation) into candidate notes, digests, and [handoffs](../glossary.md#handoff).

- **Session boundaries**: Triggered at checkpoints (`Stop`, `PreCompact`) and session termination (`SessionEnd`).
- **Scope resolution**: Sessions touching multiple projects distill each attachment independently; indeterminate cohorts stay personal.
- **Handoff generation**: Preserves explicit `next:` and `todo:` steps and records the final execution outcome of test commands. Short chats or automated sessions (`CAIRN_SESSION_ORIGIN=automation`) do not overwrite useful developer handoffs.
- **Observation privacy**: Raw observations expire after 30 days and are never exposed to memory search.

## Durable jobs

Asynchronous background workers manage compute-intensive tasks via a PostgreSQL job queue:

| Job name | Responsibility | Trigger |
| --- | --- | --- |
| `distill` | Parses raw observations into candidate notes, handoffs, and project digests | Checkpoint or SessionEnd |
| `embed` | Computes 384-dimensional vector embeddings with local MiniLM | Memory save or update |
| `link` | Traverses entity references to establish graph edges between memories | New or revised memory |
| `consolidate` | Identifies near-duplicates, applies proof counts, and suggests merges | Nightly or batch sweep |

- **Leases & backoff**: Workers acquire jobs with time-bounded leases and retry with exponential backoff.
- **Quarantine**: Jobs failing repeatedly are quarantined. Administrators can review or retry them in the console.

See [Data model](data-model.mdx) and [Main flows](index.mdx#main-flows).
