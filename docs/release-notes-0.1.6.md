---
id: CM-REL-002
title: "Cairn Memory 0.1.6 release notes"
status: draft
---

# Cairn Memory 0.1.6 release notes

Design record: [Architecture decisions](./architecture.md#decisions-and-why).

## What changed for users

- **5 MCP tools** instead of 38: search, get, save, feedback, forget. Much less tool text per session.
- **Rust hooks.** Sessions start with a short, bounded memory brief and agents get relevant memory
  while they work. No Python needed.
- **Automatic memory care.** Cairn links, merges near-duplicates, marks stale memories and shares
  repeated lessons globally by itself. There is no review queue and nothing to approve.
- **Better recall** with a small local embedding model (all-MiniLM-L6-v2, 384 dimensions, runs inside the server).
  Benchmark (43 questions among 2,000 unrelated memories): hit@5 0.65, 14 of 14 off-topic queries return nothing,
  search reply ~133 tokens, session overhead ~1,160 tokens. Recall of pure paraphrases (no shared word) stays low (4 of 17).
  Final run on the release code (`bench/after-0.1.6-final-v2.json`): hit@5 0.651, MRR@10 0.608, precision@5 0.429,
  14 of 14 off-topic queries empty, search reply ~133 tokens, session overhead ~1,162 tokens. On the 70-memory
  fixture set (`bench/after-0.1.6-final-v1.json`): hit@5 0.92, MRR@10 0.86, 5 of 5 off-topic queries empty.
  The untouched holdout set (10 questions, reported once, never used to tune) scores hit@5 0.20 with the
  2,000 distractors and 0.30 without; 4 of 5 and 5 of 5 of its off-topic queries return nothing.
- **New web console:** Dashboard, Memories, Activity, Projects, Graph, Settings. Device health,
  setup command, export and import of your own memories.
- Tested for Claude Code, Codex and OpenCode.
- The server URL for `cairn setup` can be pasted with or without a trailing `/cairn-memory`.

## Breaking changes

- **Data wipe.** 0.1.6 uses a new database schema. There is no migration; the server refuses to start on an old database, so deploy it against a fresh one.
- **Server and client must upgrade together.** Old clients cannot talk to a 0.1.6 server.
- **Python hooks are removed.** `cairn setup` installs the Rust hooks.
- **MCP tools 38 → 5.** Removed tools (recall, context, reflect, lessons, session and link tools,
  project tools, list, graph, export, diagnose, delete, ...) are gone; use `memory_search` with a `kind` filter instead.
- Admin requeue/discard of jobs and memory feedback buttons are removed from the web.
- Failed jobs that stopped retrying are tried again once after an upgrade and then every 24 hours; nobody requeues them.
- Sign-in has no rate limit or lockout (a password guess costs one Argon2 check). Put a limit in your reverse proxy if the console is public.
- Set `CAIRN_PUBLIC_URL` on the server (for example `https://cairn.example.test/agent`) to pin the `cairn setup` address the console shows. Without it the console passes the browser's own address to the server, which adds `/agent` for a public host.

## Known limitations (planned for 0.1.7)

- No CI job builds the server and web Dockerfiles on pull requests; the images are first built when a tag is pushed.
- The optional `CAIRN_SCHEMA` variable (PostgreSQL schema, default `cairn`) is not yet documented in the deploy guide.
- `apps/web/package.json` still says version 0.1.0.
- The "Agents / Hook" paragraph of the deploy guide and the hook-v1 contract may still contain Python-era wording.

## Upgrade steps (one machine)

1. Deploy the 0.1.6 server with a **fresh database**.
2. Update the client: `cairn update`.
3. Configure your agent: `cairn setup <url> --agent claude` (or `codex`, `opencode`).
4. Remove the legacy lines that source `~/.config/cairn/env` from `~/.zshrc` and `~/.zprofile`.
5. Start a new agent session in a repository (run `cairn init` there if it is new).
6. Run `cairn doctor` and check that every item passes.

Repeat steps 2–6 on each machine.
