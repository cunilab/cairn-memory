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
| **Dashboard** | Overview of system health, active sessions, and recent activity. |
| **Memories** | Search and filter memories by trust, scope, and status. Inspect evidence, counters, and pins. |
| **Skills** | Browse the shared global skill library, view version histories, and roll back revisions. |
| **Projects** | Manage project settings, view members, browse branches, and inspect session handoffs. |
| **Graph** | Visualize relationship links (`supersedes`, `contradicts`, `related`) between memories. |
| **Activity** | Inspect raw capture events, background processing jobs, and account audit logs (admin only). |
| **Fixes** | Automated health detector diagnosing configuration drift, stale pins, and updates. |
| **Settings** | Manage personal device tokens and exports. Administrators manage accounts and global agent prompts. |

*Session save counts track new records created during a session window. Project owners and administrators can edit project-specific instructions directly on the project page.*

## Fixes

The **Fixes** panel highlights detected system issues and provides one-click remediation:

- **Apply or ignore**: Administrators can apply fixes directly or dismiss false positives.
- **Device fixes**: Apply locally via `cairn fixes apply [id]`, `cairn doctor`, or `cairn init`.
- **Capabilities**: Fixes resolve unmigrated settings, tune down unused memories, update out-of-date integrations, or reverse incorrect automatic merges.

See [Accounts and authentication](../technical/security.md) and [Trust and corrections](memory/trust.md).
