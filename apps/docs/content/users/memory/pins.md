---
id: CM-DOC-USERS-MEMORY-PINS
title: "Pins"
status: accepted
description: "Pin an owned memory so it leads the session brief."
---

# Pins

A [pin](../../glossary.md#pin) puts an owned personal or project memory at the front of the opening [session brief](../../glossary.md#session-brief).

## Pin management

- **Web console**: Open any owned personal or project memory and toggle the **Pin** action.
- **API endpoint**: `POST /cairn-memory/memories/{id}/pin` with `{"pinned": true|false}`.

## Quotas and rules

| Scope | Quota limit | Shared with |
| --- | --- | --- |
| **Personal** | Up to 3 pins per account | Owner only |
| **Project** | Up to 4 pins per project | All project members |

- **Brief priority and layout**: L0 splits its 900-byte budget (600 bytes on legacy clients) evenly between personal and current-project pins (oldest first within each scope), with unused room spilling over to the other side.
- **Overflow visibility**: When all eligible pins cannot fit in L0, the brief reports `"N more pinned not shown"`, and the web console displays both quota usage and how many pins fit.
- **Existing pins over cap**: Pins saved above the quota before updating stay pinned until unpinned; new pins are blocked once a quota is reached.
- **Bypass muting**: Pinned memories are exempt from automatic muting.
- **Exclusions**: Archived projects, forgotten memories, and superseded records are excluded from L0. A superseded pin retains its slot until unpinned.
