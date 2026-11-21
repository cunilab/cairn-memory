---
id: CM-REL-002
title: "Cairn Memory 0.1.6 release notes"
status: draft
---

# Cairn Memory 0.1.6 release notes

Design record: [ADR 0015](./adr/0015-0.1.6-memory-model-and-agent-surface.md).

## What changed for users

- **5 MCP tools** instead of 38: search, get, save, feedback, forget. Much less tool text per session.
- **Rust hooks.** Sessions start with a short, bounded memory brief and agents get relevant memory
  while they work. No Python needed.
- **Automatic memory care.** Cairn links, merges near-duplicates, marks stale memories and shares
  repeated lessons globally by itself. There is no review queue and nothing to approve.
- **Better recall** with a small local embedding model (model: TODO). Benchmark: hit@5 TODO,
  search reply ~TODO tokens, session overhead ~TODO tokens (final numbers TODO).
- **New web console:** Dashboard, Memories, Activity, Projects, Graph, Settings. Device health,
  setup command, export and import of your own memories.
- Tested for Claude Code, Codex and OpenCode.

## Breaking changes

- **Data wipe.** 0.1.6 uses a new database schema. There is no migration; the server refuses to start on an old database, so deploy it against a fresh one.
- **Server and client must upgrade together.** Old clients cannot talk to a 0.1.6 server.
- **Python hooks are removed.** `cairn setup` installs the Rust hooks.
- **MCP tools 38 → 5.** Removed tools (recall, context, reflect, lessons, session and link tools,
  project tools, list, graph, export, diagnose, delete, ...) are gone; use `memory_search` with a `kind` filter instead.
- Admin requeue/discard of jobs and memory feedback buttons are removed from the web.

## Upgrade steps (one machine)

1. Deploy the 0.1.6 server with a **fresh database**.
2. Update the client: `cairn update`.
3. Configure your agent: `cairn setup <url> --agent claude` (or `codex`, `opencode`).
4. Remove the legacy lines that source `~/.config/cairn/env` from `~/.zshrc` and `~/.zprofile`.
5. Start a new agent session in a repository (run `cairn init` there if it is new).
6. Run `cairn doctor` and check that every item passes.

Repeat steps 2–6 on each machine.

## TODO before release

- Embedding model name and final benchmark numbers.
- Confirm the ADR 0015 "telescopic layout" wording.
