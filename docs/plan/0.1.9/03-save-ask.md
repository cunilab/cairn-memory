---
id: CM-PLAN-019-03
title: "03 Ask once to save 0-3 notes"
status: frozen
---

# 03 Ask once to save 0-3 notes

## Goal

When a session that changed files stops, the host agent is asked once, in about 40 tokens, to save 0–3 notes in a
single `memory_save` call. The text is editable and can be disabled through the existing agent instructions.

## Out of scope

A 6th MCP tool. Asking the agent to write the handoff (the handoff stays rule-based, item 05). OpenCode continuation
(its plugin cannot continue a turn; unchanged).

## Decisions it relies on

D-LLM (host is the extractor), D-TOK, D-TOOLS, D-ANY.

## Design

- **Trigger** (`Hook::stop_nudge`, `crates/client/src/hook/mod.rs`): fires once per session when
  `!s.edited.is_empty()` (changed files, including item-05 checkpoint files), not `s.saved`, not `s.nudged`, not
  `stop_hook_active`, not OpenCode, not automated/subagent (item 12), and nudging is on. `worked_signal` alone (a test
  run or a `Decision:` line without edits) no longer triggers it. Item 06 adds the correction case.
- **Text**: new built-in `DEFAULT_STOP_NUDGE` in `crates/core/src/instructions.rs`, ≤ 170 bytes (~40 tokens):
  `You changed {files} file(s). If you learned something reusable, call memory_save once with items: 0-3 notes (decision/gotcha/rule/fact), each with a reason. Else just stop.`
  `fill_stop_nudge` unchanged. Global and per-project overrides (0.1.9 editable instructions, `crates/store/src/instructions.rs`)
  keep working; an empty override still means "next level".
- **Disable**: a stored text of exactly `off` (global or project) disables the ask; `validate_instruction` accepts it,
  `inject::server_text` (`crates/client/src/hook/inject.rs`) maps it to "no text". `nudge:false` / `CAIRN_NO_NUDGE=1`
  still disable everything. The web editor shows the `off` hint (`apps/web/components/agent-instructions-editor.tsx`).
- **One call, many notes**: `memory_save` (`crates/client/src/tools.rs:45`) gains optional
  `items: array (maxItems 3) of the same object shape` (kind, title, summary, reason, fields, evidence, supersedes,
  contradicts, scope; aliases added by item 09). When `items` is given, top-level note fields must be absent.
  The `/remember` route (`crates/server/src/routes/memory.rs`) saves each item independently with the existing
  `Store::save_from` and answers `{"results":[{"id":..,"status":"saved|duplicate|error","error":..}]}`; one invalid item
  does not block the others. `cairn save` CLI accepts a JSON file with `items`. Tool description grows by one sentence:
  `items: up to 3 notes in one call.`
- Saves via `items` set `s.saved` like a single save (the PostToolUse matcher already sees `memory_save`).

## Allowed files

- `crates/core/src/instructions.rs`
- `crates/client/src/hook/mod.rs`, `crates/client/src/hook/inject.rs`, `crates/client/src/tools.rs`, `crates/client/src/mcp.rs`
- `crates/server/src/routes/memory.rs`, `crates/server/src/routes/instructions.rs`
- `apps/web/components/agent-instructions-editor.tsx`
- tests: `crates/server/tests/hook.rs`, `crates/server/tests/instructions.rs`, `crates/server/tests/agent_flow.rs`, `scripts/smoke_mcp.sh`

## Acceptance checks

1. A session with one edited file and no save gets exactly one ask at the first Stop; the second Stop gets nothing.
2. A session that only ran tests (no edits) gets no ask.
3. A session that saved before Stop gets no ask.
4. Built-in text ≤ 170 bytes; with the JSON wrapper ≤ 230 bytes (asserted in a test).
5. Project override `off` → no ask; global `off` with a project text → project text wins (existing precedence).
6. `memory_save` with `items` of 3 valid notes saves 3 rows and returns 3 results; with one invalid item returns 2 saved + 1 error.
7. `items` with 4 entries is rejected with a validation error; `items` plus a top-level `title` is rejected.
8. The MCP tool list still has exactly 5 tools; the `memory_save` schema grows by ≤ 450 bytes.

## Tests to write first

- `hook.rs::ask_once_only_after_file_changes` (1, 2, 3)
- `instructions.rs::stop_text_fits_budget` in `crates/core/src/instructions.rs` (4)
- `instructions.rs::off_disables_the_ask` in `crates/server/tests/instructions.rs` (5)
- `agent_flow.rs::save_items_partial_success` (6, 7)
- `mcp.rs` unit test `five_tools_and_schema_budget` (8)

## Data-safety notes

No schema change. Each item uses the normal save path (dedup, consolidate).

## Token cost

Ask: ~40 tokens once per working session (was ~50). Schema: ≤ ~110 tokens added to every session's tool list. Output:
0–3 notes written by the agent, ~150–600 output tokens, only when it has something.

## Risks

Agents may over-save; capped at 3 and new agent saves go through dedup and consolidate (item 11).

## Open questions

None.
