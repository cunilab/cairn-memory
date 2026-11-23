---
id: CM-PLAN-019-18
title: "18 Cairn is the agent's memory (session-start instruction)"
status: frozen
---

# 18 Cairn is the agent's memory (session-start instruction)

## Goal

Agents keep their built-in memory switched on, but treat Cairn as the source of truth: they search and save memory in
Cairn, not in their own local memory files. This is done only with a clear instruction at session start. No hook
blocks anything and nothing is copied (user decision 2026-11-23, replacing the earlier mirror design).

## Out of scope

Blocking or redirecting file writes. Copying agent memory files into Cairn. Editing or turning off any agent's
built-in memory. Any LLM.

## Decisions it relies on

D-LLM, D-ANY, D-TOK. User 2026-11-23: "B, but don't make the hook block; make a clear instruction when the first hook
session starts. I want Cairn as the source of truth for AI agent memory."

## Design

- **Default session instruction** (`crates/core/src/instructions.rs`, `DEFAULT_SESSION_INSTRUCTION`), replaced with
  this text (≤ 280 bytes):
  `Cairn is your memory and the source of truth. Use it instead of your built-in memory files: memory_search before deciding or debugging; memory_save (with a reason) after a decision, fixed gotcha, correction or finished task. Do not write memory to local files.`
  The unit test bound for the default moves from 200 to 280 bytes.
- **Every session gets it,** not only sessions in a Cairn project: `crates/client/src/hook/mod.rs::start` today shows
  the instruction only for a project (else `NO_PROJECT_HINT` once). Change: outside a project the instruction is
  shown too (saves go to personal scope); `NO_PROJECT_HINT` stays, once per session, after it. When the server is
  unreachable the built-in default is still shown (today only for a project file).
- **MCP instructions** (`crates/client/src/tools.rs::instructions`): add one sentence, within its existing byte test:
  `Cairn is the source of truth for memory; do not keep memory in local files.`
- **Web:** the instructions editor shows the new default where it shows defaults today; regenerated types if any.
- An admin's saved global or per-project instruction still wins over the default (unchanged behaviour).

## Allowed files

- `crates/core/src/instructions.rs`, `crates/client/src/hook/mod.rs`, `crates/client/src/tools.rs`
- `crates/client/src/mcp.rs` and `crates/client/tests/*` snapshots only if a byte or text snapshot changes
- `apps/web/lib/agent-instructions.ts` (+ its test) only if it holds a copy of the default; `apps/web/lib/generated/*`
- tests: unit tests in the files above, `crates/server/tests/hook.rs`
- `scripts/bench_write.py` only if W8's measured bytes need the new line accounted for (the budget itself stays)
- `docs/plan/0.1.9/README.md` checklist tick

## Acceptance checks

1. A session in a Cairn project gets the new instruction (default) after the brief.
2. A session outside any project (with and without git) gets the same instruction, then the no-project hint once.
3. Server unreachable: the built-in default instruction is still shown, in and outside a project.
4. A saved global or per-project instruction from the web still replaces the default.
5. MCP `initialize` instructions contain the source-of-truth sentence and stay within their byte test.
6. Item 07 bench: W1–W8 pass; W8 bytes reported before and after.

## Tests to write first

- `instructions.rs::default_says_source_of_truth` (1)
- `hook.rs::instruction_outside_project` (2, 3)
- `hook.rs::saved_instruction_overrides_default` (4)
- `mcp.rs` instructions test (5)

## Data-safety notes

No schema change, no data change.

## Token cost

About +130 bytes per session start compared with the old default, and about +70 bytes once in the MCP instructions.
Sessions outside a project gain the instruction line (about 260 bytes) they did not get before.

## Risks

An agent may still follow its built-in memory instructions; the session-start line is the only lever, by the user's
choice. Fixes F1 later counts sessions with work and no save, which shows whether it works.

## Open questions

None.
