---
id: CM-DOC-USERS-AGENTS-CLAUDE-CODE
title: "Claude Code"
status: accepted
description: "Set up Claude Code hooks and MCP, and optionally import existing project memories."
---

# Claude Code

## Connect Claude Code

```sh
cairn setup https://your-server.example --agent claude
cairn doctor
```

Supply the device token on stdin. On a configured machine use `cairn setup claude`. Restart Claude Code and check Cairn in `/mcp`. The managed instruction is written to the Claude Code config directory's `CLAUDE.md`.

Setup and update add `mcp__cairn` to `permissions.allow` when neither it nor the server-wide wildcard is present. Removal deletes the bare rule and retains the wildcard and permission containers.

## Native memory

Claude Code setup matches Bash and the four file-writing tools `Write`, `Edit`, `MultiEdit` and `NotebookEdit`. The local write guard denies those tools under the Claude config directory's `projects/*/memory/`. Use `cairn setup <url> --agent claude --keep-native-memory` to keep native writes enabled across updates while keeping the Bash warning hook. [Claude Code hook decisions](https://code.claude.com/docs/en/hooks#pretooluse-decision-control)

### Import Claude Code memory

Run `cairn import claude-memory` from the project to preview the files, scopes, and titles, then confirm the import and index replacement together. Pass `--yes` to apply directly. Cairn looks in the Claude config directory's `projects/<encoded project>/memory/`. It uses the main checkout for Git worktrees, respects `CLAUDE_CONFIG_DIR` and its optional `CLAUDE_CODE_PROJECT_DIR_NAME`, and uses the default layout for auto memory. [Claude Code memory directory](https://code.claude.com/docs/en/memory#where-auto-memory-is-stored) [Claude Code project directory names](https://code.claude.com/docs/en/sessions#name-the-project-directory-yourself)

| Claude file type | Cairn kind | Scope |
| --- | --- | --- |
| `user` | `preference` | Personal |
| `feedback` | `rule` | Personal |
| `project` | `note` | Project when `.cairn/project.json` exists, otherwise personal |
| `reference` | `fact` | Personal |

The import lists possible-secret files it skips and prints any coaching hints for saved files. The normal save API deduplicates reruns. Cairn stores the body in `detail`, uses the `**Why:**` line or the first 1,000 body characters as the reason, and keeps the source path as evidence. It lists bodies clipped to the 8,000-character detail limit and includes a pointer to the full text for each. Once every eligible file saves, Cairn backs up the old `MEMORY.md` as `MEMORY.md.pre-cairn` and writes the pointer. The original note files stay on disk. If a save fails, fix the reported file and rerun. Any files already saved will deduplicate.

[Claude Code hook details](../../technical/hooks/claude-code.md).
