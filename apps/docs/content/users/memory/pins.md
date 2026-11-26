---
id: CM-DOC-USERS-MEMORY-PINS
title: "Pins"
status: accepted
description: "Pin an owned memory so it leads the session brief."
---

# Pins

Pin essential personal or project memories to lead the opening session brief.

## Pin management

- **Web console**: Open any owned personal or project memory and toggle the **Pin** action.
- **API endpoint**: `POST /cairn-memory/memories/{id}/pin` with `{"pinned": true|false}`.

## Quotas and rules

| Scope | Quota limit | Shared with |
| --- | --- | --- |
| **Personal** | Up to 5 pins per account | Owner only |
| **Project** | Up to 10 pins per project | All project members |

- **Brief priority**: Eligible pins appear oldest first within the [L0 budget](layers.md#automatic-recall).
- **Bypass muting**: Pinned memories are exempt from automatic muting.
- **Exclusions**: Archived projects, forgotten memories, and superseded records are excluded from L0. A superseded pin retains its slot until unpinned.
