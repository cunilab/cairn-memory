---
id: CM-DOC-USERS-MEMORY-PINS
title: "Pins"
status: accepted
description: "Pin an owned memory so it leads the session brief."
---

# Pins

Open an owned personal or project memory in the web console and use its pin action. Use the same sheet to unpin it.

Owners can pin or unpin memories in the web memory sheet. The web route is `POST /cairn-memory/memories/{id}/pin` with a Boolean `pinned` field. A person can pin up to five personal memories and a project holds up to ten pins across its members. Pins bypass muting. Archived project, forgotten and superseded memories stay out of L0. A superseded pin retains its slot until unpinned so restoring a correction preserves the quota. Forgetting a memory frees its slot.

The brief shows eligible pins oldest first within the [L0 budget](layers.md#automatic-recall). Pinning a project memory shares it through the project with members who open a session there.
