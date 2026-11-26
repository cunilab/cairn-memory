---
id: CM-REF-003
title: "Projects and memory scopes"
status: draft
description: "Understand visibility, membership, and what happens when you archive or restore a project."
---

# Projects and memory scopes

A memory’s [scope](../../glossary.md#scope) determines who can see it.

## Memory scopes and record

- **[Personal](../../glossary.md#personal-scope)**: Visible only to the creating account. Preferences, session handoffs, and draft notes default to personal scope.
- **[Project](../../glossary.md#project-scope)**: Visible to all registered members of the project. Decisions, rules, gotchas, facts, and procedures default to project scope when the caller belongs to the project.
- **[Global](../../glossary.md#global-scope)**: Visible to all authenticated accounts on the Cairn server. Used for shared skills and server-wide instructions.
- **Retention**: Memories never expire by age. Forgotten memories are excluded from search; stale and superseded records remain retrievable with lower ranking and a label.

See [CLI and MCP](../reference/mcp.md) for tool interfaces.

## Init and membership

Initialize a project directory and commit `.cairn/project.json`:

```sh
cairn init "Project name"
```

- **Project ID is authority**: Anyone possessing the committed project ID can join and access project memories. A Git remote alone does not grant membership.
- **Single-remote attachment**: A checkout lacking a `.cairn/project.json` file automatically attaches if its Git remote matches exactly one existing project membership for that user. Zero or multiple matching memberships default to personal scope.
- **Renaming**: Project owners rename projects in the web console. Teammates update their local file by re-running `cairn init`.
- **Ownership transfer**: Owners can transfer project ownership to any active member via the web console.

## Archive and restore

Project owners and administrators can archive inactive repositories from the project page:

- **What happens on archive**:
  - Project memories are hidden from agent retrieval, briefings, and topic notes.
  - Members can still view records in read-only mode under **Projects → Archived**.
  - Memory IDs, links, history, and memberships are completely preserved.
- **Restore**:
  - Click **Restore** under **Projects → Archived** to immediately re-enable agent search and resume queued processing.
