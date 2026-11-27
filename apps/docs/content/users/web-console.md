---
id: CM-DOC-USERS-WEB-CONSOLE
title: "Web console"
status: accepted
description: "Find memories, skills, sessions, jobs, device settings and administrator Fixes."
---

# Web console

Browser interface for inspecting memories, managing projects and devices, reviewing audit logs, and applying fixes.

The [glossary](../glossary.md#web-console) defines the records and tools you see in the console.

## Navigate the console

| Section | Purpose |
| --- | --- |
| **Overview** | What needs attention, memory health, and recent usage. |
| **Memories** | Exact phrase search by default, with a Related mode. All and Pinned tabs, row pins, and numbered pages. |
| **Projects** | Project rows, then a project overview, memories, sessions, and settings. |
| **Skills** | Library, Suggestions, and Archived. |
| **Activity** | System events, account audit, and background jobs. |
| **Relationships** | Desktop graph and a relationship list. Phones use the list. |
| **Fixes** | Admin queue grouped by who can act next. Open from Overview or the admin section. |
| **Settings** | Your account (devices, profile, data) and administration (accounts, agent instructions). |

Search matches the exact phrase in one field and highlights it. Pinned shows personal and project quotas (3 and 4). Pages are numbered; the list does not append with Load more.

*Session save counts track new records created during a session window. Project owners and administrators can edit project-specific instructions directly on the project page.*

A memory sheet with no detail says "No detail". Administrators add or edit that text from **Rewrite memory** (up to 8,000 characters). Saving a rewrite, including a detail-only edit, stores a new version. An empty detail clears it on that version. The previous version keeps the detail it had.

## Fixes

The **Fixes** panel highlights detected system issues and provides one-click remediation:

- **Apply or ignore**: Administrators can apply fixes directly or dismiss false positives.
- **Device fixes**: Apply locally via `cairn fixes apply [id]`, `cairn doctor`, or `cairn init`.
- **Capabilities**: Fixes resolve unmigrated settings, tune down unused memories, update out-of-date integrations, or reverse incorrect automatic merges.

See [Accounts and authentication](../technical/security.md) and [Trust and corrections](memory/trust.md).
