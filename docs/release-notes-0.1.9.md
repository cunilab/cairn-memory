---
id: CM-REL-005
title: "Cairn Memory 0.1.9 release notes"
status: draft
---

# Cairn Memory 0.1.9 release notes

Agents are now reminded to use Cairn, and the reminders are editable in the web. A retrieval-quality investigation
raised the CI bench floor to 0.65, two review fixes landed, and **retrieval itself did not change**. One additive
migration, no new tools.

## What changed for users

- **Agent nudges.** In a repository with `.cairn/project.json`, the hook now tells the agent to use Cairn (Kivori ran 9
  sessions and saved nothing, because agents ignore the MCP instructions):
  - *Session start line.* One instruction line is added after the brief: "Cairn is active for this project. Before
    deciding or debugging, memory_search; after a decision, fixed gotcha or finished task, memory_save with a reason."
    It is given even when the project has no brief yet or the server cannot be reached.
  - *Stop reminder.* The first `Stop` of a session that edited files, ran a recognised test command or wrote a
    `Decision:` line, and made no Cairn save, answers `{"decision":"block","reason":...}`, so Claude Code (and Codex,
    which follows the same Stop contract; untested against a live Codex) continues for one turn. Once per session,
    never when the host reports `stop_hook_active`, never for a session of questions only. OpenCode's plugin cannot
    continue a turn, so it gets the start line only.
  - *Saves are tracked.* The Cairn `memory_save` tool (any host) or `cairn save` counts as a save. The Claude Code and
    Codex `PostToolUse` matcher now also lists `mcp__cairn__memory_save`: **re-run `cairn setup --agent <agent>` once**
    so the hook sees MCP saves (the `cairn update` that installs 0.1.9 rewrites the hooks with the old matcher, a later
    one with the new); until then the reminder can fire after a save.
  - *Opt out.* `"nudge": false` in `~/.config/cairn/config.json` (kept by a later `cairn setup`) or
    `CAIRN_NO_NUDGE=1`.
  - `bun test` and `bunx vitest` are now recognised test commands (`npm test`, `pnpm test`, `yarn test` already were).
- **Editable agent instructions.** Both texts are server settings, at most 600 bytes each, with `{files}` in the
  Stop text replaced by the number of edited files. Per field the order is: the project's override, then the global
  text, then the built-in text; an empty text clears.
  - Global: Settings, **Agent instructions**; a server admin edits, everyone reads.
  - Per project: the project page, **Agent instructions**; the project's owner (or a server admin) edits, members read.
  - Each tab shows the source in use, byte counters, a preview of what the agent receives, Save, and Reset to default
    (Clear override on a project). Every change writes an `agent_instructions_changed` audit event with the old and
    new text (Activity).
  - The texts ride in the `/session/brief` reply (`instructions`), and the hook keeps the Stop text in its session
    state, so there is no extra request at Stop. An unreachable server or an empty text means the built-in text.
  - The MCP `initialize` instructions stay static in this release.
- **`cairn init` no longer silently takes ownership of a lost project.** When `.cairn/project.json` names an id the
  server has never heard of (a wiped or new server), whoever registers it first becomes its owner. `cairn init` now
  refuses with a warning and tells you to run `cairn init --yes` if that is intended. Joining a project the server
  knows is unchanged.
- **Fewer hook requests.** A session asks the server for topic notes at most 16 times (found or not; a path is still
  asked once). Before, a session that opened hundreds of files with no notes made one request per file.

### Tokens added by the nudges

| | bytes | tokens (bytes / 4) | when |
|---|---|---|---|
| start line (built-in text) | 154 (+1 newline) | about 39 | once per session start, in an initialised repository |
| Stop reminder (built-in text, with the JSON wrapper) | about 200 | about 50 | at most once per session, only after work with no save |
| bench scripted session (one start, ten edits) | 1,466 to 1,621 | 366 to 405 | measured, see below |

A custom text costs its own size, at most 600 bytes (150 tokens) per field. The `/session/brief` reply also grows by
the `instructions` object, which is HTTP only and not model context.

## Retrieval and the bench floor

We looked for a general retrieval fix to raise the CI `Bench` floor from hit@5 0.60 to 0.68 (target 0.72) and found
none that keeps weak queries silent. Details: [`bench/analysis-0.1.9.md`](../bench/analysis-0.1.9.md).

- All 14 v2 misses are paraphrases with no shared word. Every gold the gate keeps is already in the top 5; 10 misses
  have a best candidate under the 0.43 cosine gate, 4 are crowded out by distractors that pass it.
- The off-topic (weak) queries reach cosine 0.405, above the recoverable golds (0.33 to 0.40), so no threshold, gap or
  query rewrite gains a hit without returning cards for an off-topic question. Stripping stopwords from the query was
  measured and gave nothing.
- The floor is therefore **0.65** (was 0.60): the run is deterministic at 0.674, a margin of 0.024. 0.68 would fail today.

| bench v2, 2,000 distractors | 0.1.8 (before) | 0.1.9 (after) |
|---|---|---|
| hit@5 | 0.674 | 0.674 |
| MRR@10 | 0.663 | 0.663 |
| precision@5 | 0.460 | 0.460 |
| weak queries returning nothing | 14/14 | 14/14 |
| holdout hit@5 / MRR | 0.200 / 0.125 | 0.200 / 0.125 |
| bench v1 hit@5 / MRR | 0.92 / 0.92 | 0.92 / 0.92 |
| bytes per search (mean) | 523 | 523 |
| hook injection, scripted session | 1,466 B (366 tok) | 1,621 B (405 tok), the start line |

`scripts/bench.py --dump-scores` now also records the five highest candidate cosines per query.

## Breaking changes

- `POST /cairn-memory/projects/init` with an unknown `project_id` answers 409 unless the body has
  `confirm_register: true`. `cairn init --yes` sends it. A client older than 0.1.9 that hits this case gets the 409
  message instead of a silent registration.
- The `/session/brief` reply has a new `instructions` object. Clients that ignore it are unaffected.
- Migration `0006_agent_instructions` adds two nullable columns to `projects` (`session_instruction`, `stop_nudge`);
  it applies at server start and needs no data change. The global texts use the existing `server_state` table.

## Upgrade steps

1. Upgrade the server and the clients together. The server applies the migration at start; there is no data wipe.
2. On each machine run `cairn update`, then `cairn setup --agent <agent>` once for each agent you use (so the hook
   sees Cairn saves through the MCP tool).
3. Run `cairn doctor` and check that every item passes.
