---
id: CM-DOC-USERS-MEMORY-RECALL
title: "Find and use memories"
status: accepted
description: "Read a brief or handoff, search for a decision, and fetch the full record."
---

# Find and use memories

## Start from the brief

The session brief can include the checkout handoff, relevant preferences and project knowledge. File, command and prompt activity can trigger smaller replies later in the session. [Recall layers](layers.md) shows their delivery limits.

## Search explicitly

Ask the agent to call `memory_search`. Include the distinctive decision, command, file name or version. Use `kind: handoff` to find continuity records. For a known ID, call `memory_get` and request detail if the title, summary and reason are insufficient.

## When a record does not appear

Confirm that the account can see the record and belongs to the expected project. Check that the project is active and that the current checkout attaches to it. Query with an exact term from the record to test the relevance gate. Check [semantic search health](../help/troubleshooting.md) if paraphrases miss. Use search or get to restore automatic eligibility for a muted card.
