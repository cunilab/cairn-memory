---
id: CM-DOC-USERS-MEMORY-KINDS
title: "Kinds and records"
status: accepted
description: "Select the right record type and supply the fields that help agents reuse it."
---

# Kinds and records

Every authored memory requires a `kind`, `title`, and `summary`. A `reason` is required for every kind except `note`. Add [`evidence`](../../glossary.md#evidence) (source files/URLs) and kind-specific fields to maximize reuse value.

Optional `detail` is longer text, at most 8,000 characters. Agents leave it out of ordinary reads and fetch it with `memory_get` (`detail: true`). The web console shows it on the memory sheet. An administrator can add or edit it there; an empty detail removes it.

## Memory kinds

| Kind | Purpose | Example | Kind-specific fields |
| --- | --- | --- | --- |
| **[Decision](../../glossary.md#decision)** | Architectural or design choice and rationale | *Use disposable schemas for integration tests* | `context`, `alternatives`, `consequences` |
| **[Rule](../../glossary.md#rule)** | Hard project constraint or guideline | *Preserve existing migration files* | `exceptions` |
| **[Gotcha](../../glossary.md#gotcha)** | Verified failure symptom and fix | *Setup targeted wrong profile* | `symptom`, `cause`, `fix` |
| **[Procedure](../../glossary.md#procedure)** | Step-by-step repeatable workflow | *Rotate a device credential* | `steps`, `when` |
| **[Fact](../../glossary.md#fact)** | Verified environment or configuration property | *Server listens on port 3111 by default* | `source` |
| **[Preference](../../glossary.md#preference)** | Personal working style or habit | *Ask before replacing a provider* | `example` |
| **[Note](../../glossary.md#note)** | General context without specific structure | *Notes from a migration review* | None |
| **[Handoff](../../glossary.md#handoff)** | System-generated session continuity | *Resume after the last test run* | None (system-managed) |

## Linked relations

- **`supersedes`**: Reference an existing memory ID when [superseding](../../glossary.md#superseded) an outdated record. The previous record is marked superseded and preserved in history.
- **`contradicts`**: Declare a contradiction against another record you cannot replace directly.
- **Inspect record**: Call `memory_get` with an ID to view scope, trust rating, counters, timestamps, and relationship links.

See [Tool arguments and compact responses](../reference/mcp.md#tools).
