---
id: CM-DOC-USERS-MEMORY-RECALL
title: "Find and use memories"
status: accepted
description: "Read a brief or handoff, search for a decision, and fetch the full record."
---

# Find and use memories

Strategies for consuming opening briefs, triggering contextual recall, and executing explicit searches.

## Start from the brief

At session startup, Cairn injects the opening brief:
- **Identity & Pins (L0)**: Active personal and project pins.
- **Essential Context (L1)**: Last checkout handoff, personal preferences, and core project rules, decisions, and gotchas.
- See [Recall layers](layers.md) for size caps and ordering.

## Search explicitly

When automatic context is insufficient, instruct your agent to call `memory_search`:

- **Keyword queries**: Search specific terms, file paths, tool names, or error messages (`cairn search "PostgreSQL schema"`).
- **Handoff continuity**: Pass `kind: "handoff"` to retrieve recent session continuity summaries.
- **Fetch full record**: Call `memory_get` with the returned ID. Pass `detail: true` (`cairn get <id> --detail`) to inspect long-form technical notes.

## When a record does not appear

1. **Verify scope & membership**: Ensure your account belongs to the project where the memory was saved.
2. **Check project status**: Confirm the project is active (archived projects are excluded from agent retrieval).
3. **Test lexical match**: Query with an exact title word to verify whether the semantic relevance gate dropped a paraphrase.
4. **Inspect muting**: Memories pushed three times without use are muted; fetch the record with `cairn get <id>` or vote helpful to restore automatic eligibility ([Troubleshooting](../help/troubleshooting.md)).
