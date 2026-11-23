---
id: CM-PLAN-019-18
title: "18 Agent memory files mirrored into Cairn"
status: frozen
---

# 18 Agent memory files mirrored into Cairn

## Goal

Agents keep their own built-in memory (Claude Code `~/.claude/projects/<cwd>/memory/*.md`, Codex, Gemini). It stays
on one machine and Cairn never sees it. The hook copies those files into Cairn automatically, so they reach other
devices, other agents and the team, with no extra agent tokens and without turning the built-in memory off.

## Out of scope

Turning off or editing any agent's built-in memory. Writing back from Cairn into agent files. Mirroring instruction
files (`CLAUDE.md`, `AGENTS.md`). Deleting or changing a Cairn memory when its file is deleted. Any LLM.

## Decisions it relies on

D-LLM, D-DATA, D-ANY, D-TOK, D-RANK. User 2026-11-23: built-in memory stays on; agents must also land in Cairn;
this is item 18 of 0.1.9.

## Design

- **When:** the hook scans on `SessionStart` and on `Stop`/`SessionEnd` (the same events that already checkpoint).
  A scan reads the known memory locations, keeps files whose content hash differs from the last one this device
  mirrored (state file next to the hook's existing per-device state), at most 20 files and 16 KiB each per scan.
  The first scan on a device therefore copies every existing file (the catch-up), capped per scan; the rest follow
  on later scans. Scanning (not watching tool calls) also catches files written by shell commands or by an agent's
  own memory tool.
- **Where (constant list in the client, one place to extend):**
  - Claude Code: `~/.claude/projects/<encoded cwd>/memory/*.md` for the session's cwd (and the git top when it
    differs). `<encoded cwd>` is the absolute path with every `/` and `.` replaced by `-` (verify against a real dir;
    ask if it differs). `MEMORY.md` (the index) is skipped.
  - Codex: `~/.codex/memories/**/*.md`. Gemini: the `## Gemini Added Memories` section of `~/.gemini/GEMINI.md`
    (one mirror per bullet). Missing paths are skipped silently.
- **Mapping:** frontmatter `name`, `description`, `metadata.type` (or top-level `type`) when present.
  - title = `name` (or the file stem), ≤ 80 chars; summary = `description` (or the first non-empty body line), ≤ 240;
    `reason` = the body, clipped to 1,000 chars with `… (clipped)`; evidence = `mirror:<agent>:<path with ~>`.
  - kind: `feedback` → rule, `user` → preference, `reference` → fact (`fields.source` = path), `project` or none →
    note.
  - scope: `user`/`feedback` → personal; otherwise project when the session is in a Cairn project (item 02),
    else personal. Gemini and Codex global files → personal.
  - rank: `candidate` (D-RANK); origin `agent` (an agent wrote the file). Normal redaction and validation apply;
    a file rejected by validation (e.g. a secret) is skipped and counted, never retried in a loop.
- **Updates:** one new route `POST /cairn-memory/session/mirror` takes the list of parsed files. For each, the server
  finds this owner's previous mirror of the same evidence key: same content → nothing; changed → save the new
  version with `supersedes` the old one (old kept, D-DATA); none → new memory. A forgotten previous mirror is not
  revived: the file is skipped.
- **File deleted:** nothing happens in Cairn (D-DATA).
- **Hook output:** nothing is printed to the agent (0 tokens). Failures go to the existing warn path; a down server
  leaves the hashes unchanged so the next scan retries.

## Allowed files

- `crates/client/src/hook/mirror.rs` (new: locate, parse, hash, cap), `crates/client/src/hook/mod.rs` (call on the
  named events), `crates/client/src/hook/state.rs` (per-device mirror hashes, only if it already holds device state;
  else a small file beside it in `mirror.rs`)
- `crates/server/src/routes/capture.rs` or a new `crates/server/src/routes/mirror.rs`, `crates/server/src/routes/mod.rs`
- `crates/store/src/mirror.rs` (new), `crates/store/src/lib.rs` (module only; **no migration**)
- tests: `crates/client` unit tests in `mirror.rs`, `crates/server/tests/hook.rs`, `crates/store/tests/mirror.rs` (new)
- `docs/plan/0.1.9/README.md` checklist tick

## Acceptance checks

1. A Claude memory file with frontmatter `type: feedback` becomes a personal `rule` at rank candidate with the
   evidence key; the index `MEMORY.md` is not mirrored.
2. Editing the file and scanning again creates a new version that supersedes the old one; the old one stays.
   Scanning an unchanged file sends nothing (no request when no hash changed).
3. Deleting the file changes nothing in Cairn. A forgotten mirror is not revived by a later scan.
4. A project-type file in a Cairn project gets project scope; outside a project (no `.cairn/project.json`, with or
   without git) it is personal.
5. Caps hold: 25 files → 20 mirrored in the first scan, 5 in the next; a 40 KiB file is clipped, not rejected.
6. A file containing a secret is skipped by validation and does not block the other files.
7. The hook prints nothing for the mirror (0 bytes added to any hook output); server down → hashes unchanged, next
   scan retries.
8. Works for a Gemini `GEMINI.md` bullet list and a Codex `memories/*.md` file in a temp HOME.

## Tests to write first

- `mirror.rs::parses_frontmatter_and_maps_kind_scope` (1, 4)
- `mirror.rs::encoded_claude_dir_for_cwd` (1)
- `mirror.rs::caps_and_hash_skip` (2, 5, 7)
- `store/tests/mirror.rs::new_changed_same_forgotten` (2, 3, 6)
- `server/tests/hook.rs::mirror_route_member_and_owner_only` (route auth like the other session routes)
- `mirror.rs::gemini_and_codex_sources` (8)

## Data-safety notes

No schema change. Only inserts and supersede links; nothing is deleted or overwritten. The agent's files are only read.

## Token cost

0 for agents: nothing is injected. Mirrored memories show up in search and the brief like any other candidate note.

## Risks

Personal notes of one machine become visible to the owner's other devices (intended). A project-type note becomes
visible to project members: same rule as any project save. Junk files rank as candidates until used.

## Open questions

None.
