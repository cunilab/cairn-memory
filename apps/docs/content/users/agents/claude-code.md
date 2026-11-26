---
id: CM-DOC-USERS-AGENTS-CLAUDE-CODE
title: "Claude Code"
status: accepted
description: "Set up Claude Code hooks and MCP, and optionally import existing project memories."
---

# Claude Code

Configure Claude Code hooks and MCP, manage native memory guards, and import project notes.

## Connect Claude Code

```sh
cairn setup https://cairn.example.com --agent claude
cairn doctor
```

Supply the device token on stdin. On a previously configured device, run `cairn setup claude`.

- **Instructions**: Writes managed prompt instructions into `CLAUDE.md`.
- **Permissions**: Appends `mcp__cairn` to `permissions.allow` in the Claude Code settings.
- **Verify**: Restart Claude Code and verify that `cairn` appears active under `/mcp`.

## Native memory

By default, Cairn installs a local write guard denying Claude Code's file-writing tools (`Write`, `Edit`, `MultiEdit`, `NotebookEdit`) inside `projects/*/memory/` to prevent dual-memory conflicts.

- **Keep native writes**: Run setup with `--keep-native-memory` to allow native memory writes while retaining Bash warning hooks:
  ```sh
  cairn setup https://cairn.example.com --agent claude --keep-native-memory
  ```

### Import Claude Code memory

Migrate existing native Claude Code memory files into Cairn:

```sh
cairn import claude-memory
# Or apply immediately without prompt:
cairn import claude-memory --yes
```

| Claude file type | Cairn kind | Default scope |
| --- | --- | --- |
| `user` | `preference` | Personal |
| `feedback` | `rule` | Personal |
| `project` | `note` | Project (if `.cairn/project.json` exists), otherwise Personal |
| `reference` | `fact` | Personal |

- **Safe backup**: Backs up existing `MEMORY.md` to `MEMORY.md.pre-cairn` and writes a pointer to Cairn.
- **Deduplication**: Files with matching content deduplicate cleanly without creating redundant records.

See [Claude Code hook details](../../technical/hooks/claude-code.md).
