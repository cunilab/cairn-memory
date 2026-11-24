---
id: CM-REF-003
title: "Projects and memory scopes"
status: draft
---

# Projects and memory scopes

## Memory scopes and record

Personal memories are visible to their owner, project memories to project members, and global memories to authenticated server users. A preference is personal. A decision, rule, gotcha, procedure or fact defaults to project scope when the caller belongs to the project in context, otherwise personal; a note defaults to personal. A handoff is system-written. Age does not hide memories. Forgotten memories are hidden from normal recall; stale and superseded memories can still be found.

A save contains kind, title, summary, reason (except for notes), optional kind-specific fields, evidence, aliases, scope, and optional `supersedes` or `contradicts` IDs. The server redacts and validates it. An identical save can return the existing ID and merge evidence; a forgotten identity cannot be silently recreated. [CLI and MCP](cli-and-mcp.md) describes the tools.

## Init and membership

Run `cairn init "Project name"` in a folder and commit `.cairn/project.json`; the file contains a project ID, repository and name, not a server URL or credential. This works without Git. An existing file joins a known project. An unknown ID requires explicit confirmation with `cairn init --yes` before registration. To rename a project, the owner uses its page in the web console. Teammates see a session-start notice; run `cairn init` to update the project file and commit it.

**An ID grants membership:** anyone with an account on the server and the project ID can join and read project memories and member emails. A Git remote alone does not grant membership. Joins made using the project file are audited. Protect the file accordingly; deleting it does not remove an existing membership.

The project owner or an administrator can transfer ownership to an existing member using the web console. The former owner remains a member.
