---
id: CM-DOC-USERS-MEMORY-KINDS
title: "Kinds and records"
status: accepted
description: "Select the right record type and supply the fields that help agents reuse it."
---

# Kinds and records

Schema reference for Cairn memory types, required attributes, and kind-specific fields.

Every authored memory requires a `kind`, `title`, and `summary`. A `reason` is required for every kind except `note`. Add `evidence` (source files/URLs) and kind-specific fields to maximize reuse value.

## Memory kinds

| Kind | Purpose | Example | Kind-specific fields |
| --- | --- | --- | --- |
| **Decision** | Architectural or design choice and rationale | *Use disposable schemas for integration tests* | `context`, `alternatives`, `consequences` |
| **Rule** | Hard project constraint or guideline | *Preserve existing migration files* | `exceptions` |
| **Gotcha** | Verified failure symptom and fix | *Setup targeted wrong profile* | `symptom`, `cause`, `fix` |
| **Procedure** | Step-by-step repeatable workflow | *Rotate a device credential* | `steps`, `when` |
| **Fact** | Verified environment or configuration property | *Server listens on port 3111 by default* | `source` |
| **Preference** | Personal working style or habit | *Ask before replacing a provider* | `example` |
| **Note** | General context without specific structure | *Notes from a migration review* | None |
| **Handoff** | System-generated session continuity | *Resume after the last test run* | None (system-managed) |

## Linked relations

- **`supersedes`**: Reference an existing memory ID when updating an outdated record. The previous record is marked superseded and preserved in history.
- **`contradicts`**: Declare a contradiction against another record you cannot replace directly.
- **Inspect record**: Call `memory_get` with an ID to view scope, trust rating, counters, timestamps, and relationship links.

See [Tool arguments and compact responses](../reference/mcp.md#tools).
