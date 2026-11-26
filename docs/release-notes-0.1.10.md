---
id: CM-REL-006
title: "Cairn Memory 0.1.10 release notes"
status: draft
---

# Cairn Memory 0.1.10 release notes

This release adds layered automatic recall, memory trust and pins, a shared skill library, setup for Grok Build, Antigravity CLI and Hermes Agent, and Claude Code memory import. The agent surface grows from five MCP tools to nine: the five memory tools plus four skill tools. Token estimates below are approximate and apply only when the described context is shown to the agent.

## Fixes

- `cairn init` in a folder without a Git remote or project file now creates its own project. Before, a second folder with the same name joined the first folder's project. A folder with `.cairn/project.json` keeps its project ID.
- Setup and `cairn update` allow Cairn MCP tools. Claude Code gets `mcp__cairn` in `permissions.allow` unless a server-wide rule already exists, and Codex sets `default_tools_approval_mode = "approve"` on `mcp_servers.cairn`, which also covers `codex exec`. Removal deletes the bare `mcp__cairn` rule and keeps a wildcard rule.
- `memory_search` finds notes and handoffs that hooks captured. Hooks and agent tools send the same worktree ID derived from the canonical checkout path.

## Changes and token costs

| Area | Change | Agent token cost |
| --- | --- | --- |
| Web | Project owners and the administrator can archive a project and restore it from **Projects → Archived**. Archiving hides its memories from agents and pauses project processing; members can still read them in the console. The project page lists the project's memories. | None; web only. |
| Web | The **Skills** page lists the skill library with version history, rollback and suggested procedures. **Settings → Agent instructions** adds the Hermes chat instruction. The memory sheet can pin or unpin a memory. | None; web only. |
| Setup | `--agent grok`, `--agent antigravity` and `--agent hermes` install hooks, MCP and an instruction block. Grok delivers the brief with the first tool result; Antigravity checkpoints on Stop. `cairn setup hermes --gateway` sets up a dedicated Hermes chat profile with native search, save and skill tools. | Same brief and recall budgets as other agents. |
| Setup | Setup writes a marked Cairn block into each agent's global instruction file and refreshes it on update. | Up to about 700 bytes (~175 tokens) of instruction text in the agent's global instructions. |
| Setup | `cairn import claude-memory` previews and imports Claude Code memory files, backs up `MEMORY.md` as `MEMORY.md.pre-cairn` and writes a pointer. Claude setup denies `Write`, `Edit`, `MultiEdit` and `NotebookEdit` in Claude's memory folder; `--keep-native-memory` leaves native writes enabled. | A denied write returns one short reason. |
| Memory | Each search card and record carries a trust line: `doubted` (stale or reported wrong), `new` (candidate), `proven` (two helpful votes or three saves of the same memory) or `normal`. Cairn counts how often a memory is shown by hooks, used through search or get, and opened in full. | About 5–15 tokens per card. |
| Memory | A valid save that needs work is kept at trust `new` with up to four `hints` and an `improve` line instead of being rejected. A helpful vote confirms it; a clean correction saves at normal rank. | Hints only on a coached save, up to about 140 tokens. |
| Memory | `memory_save` accepts an optional `detail` of up to 8,000 characters. `memory_get` returns `detail_chars` and returns the text only with `detail: true` for one ID. The tool schema recommends titles of up to 10 words and one-sentence summaries of up to 30 words, and errors give the actual length. | Memory tool schemas grow from 3,225 to 3,482 bytes (~65 tokens). Detail costs tokens only when requested. |
| Recall | The session brief has two layers: L0 pins (600 bytes) and L1 essentials (1,200 bytes) ranked by trust, use and age. Brief and recall cards show the age of the original save. | Up to 600 more bytes (~150 tokens) per session start for clients that request the extended brief. |
| Recall | Automatic recall shares eight cards per person and session across the brief, file topics, command warnings, failed-command recall and prompt recall. A memory is pushed at most once per session. | Bounded by the eight-card budget. |
| Recall | A shell command can receive up to two gotchas or procedures once per normalized command per session. A failed command can recall related memories from the last 2 KiB of redacted output. | Up to 300 bytes (~75 tokens) per matching reply. |
| Recall | Prompts of at least four words can recall up to two memory cards and one skill card at 0.60 relevance or higher. | Up to 300 bytes (~75 tokens) per matching prompt. |
| Recall | After three pushes in separate sessions with no use, a memory is muted for that person's automatic recall. A search hit, get or helpful vote restores it. Pins are never muted. | Fewer repeated cards. |
| Hermes chat | Saves without a project are personal and tagged to their chat. Only that chat and the gateway account's web session see them. Cairn stores salted hashes of the chat and thread route, not person IDs, and accepts a route from any platform. Group saves start as candidates. Each chat thread, or a chat without threads, is one session that closes after 30 minutes idle. Cron, delegated and flush turns are skipped. | The Hermes chat instruction (up to 600 bytes) rides with the first prompt of each chat session. |
| Skills | A global skill library with `skill_search`, `skill_get`, `skill_save` and `skill_feedback`. Saving an existing name adds a version. The web rolls back by appending a copy of an older version. Each account has one vote per version. Procedures with steps that were fetched five times or voted helpful twice are suggested for conversion. Only administrators archive and restore skills. | Skill tool schemas add 1,500 bytes (~375 tokens) to `tools/list`; MCP instructions gain one 54-byte line. A skill card shares the prompt recall budget. |
| Doctor | `cairn doctor` warns when a managed global instruction block is missing or differs from this version, and when a Claude Code project's `MEMORY.md` lacks the Cairn pointer. | None. |

Skill limits are in UTF-8 bytes: names 1–64 lowercase letters or digits separated by single hyphens, titles 80, descriptions and change notes 240, bodies 32,000 and search queries 1,024. A person can pin five personal memories and a project holds ten pins across its members. See [recall](reference/recall.md), [CLI and MCP](reference/cli-and-mcp.md) and [hook setup](reference/hook.md#setup) for details.

## Database migrations

The server applies these migrations on startup:

| Migration | Purpose |
| --- | --- |
| `0015_memory_detail` | Optional memory detail and its lexical index. |
| `0016_project_archive` | Project archive timestamp. |
| `0017_device_agents` | Device agent CHECK widened for Grok and Antigravity. |
| `0018_memory_usage` | Shown and detail-open counters. |
| `0019_session_shown` | Per-session recall ledger. |
| `0020_device_agent_hermes` | Device agent CHECK widened for Hermes. |
| `0021_push_usage` | Per-person push and use counts, backfilled from the ledger. |
| `0022_memory_pins` | Pin timestamp. |
| `0023_hermes_chat` | Personal chat tag. |
| `0024_hermes_route_privacy` | Removes obsolete sender metadata from retained chat contexts. |
| `0025_skills` | Skills, versions, votes, suggestion dismissals and the per-session skill ledger. |

The migrations are additive with two exceptions. `0017` and `0020` replace the device agent CHECK in one statement and keep every device row; both are approved entries in `migrations/destructive-allowlist.txt`. `0024` deletes the `sender_id_hash` and `owner` keys from the chat context of retained sessions and observations; the chat and thread hashes stay.

## Upgrade

1. Back up the database. Deploy the server before the web console, because the console calls the new skill, pin and archive routes. The server applies the migrations at startup.
2. On each machine run `cairn update`, then run `cairn setup <agent>` once for each configured agent. The 0.1.9 updater refreshes integrations with its own old setup code, so `cairn update` alone does not install the new hooks, MCP permissions and instruction blocks. From 0.1.10 on, `cairn update` refreshes them with the newly installed executable.
3. In a Claude Code project with native memory, run `cairn import claude-memory` to preview and import it.
4. Run `cairn doctor` and address any failures and warnings.

Older clients keep working and receive a session brief of up to 1,200 bytes with L0 pins first. A 0.1.10 Hermes gateway checks the server version on each chat event and keeps chat memory off against an older server, which would otherwise store chat content as ordinary personal memory. Hermes built-in memory (`memories/MEMORY.md` and `USER.md`) is not imported or disabled yet; that is planned for 0.1.11.

## Known limits

- Hermes gateway chats were not tested live on Telegram or Discord.
- Grok Build, Antigravity CLI and Hermes Agent hook delivery was not verified in live vendor sessions.
- `cairn doctor` was not run natively on Windows or Linux.
- The automatic recall latency gate is opt-in and its timing on CI runners is unmeasured; see [the benchmark](../bench/README.md).
- The Claude memory write guard covers Claude's file tools. A shell command can still write to the memory folder.
