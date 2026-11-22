---
id: CM-PLAN-019-06
title: "06 User corrections become rules"
status: frozen
---

# 06 User corrections become rules

## Goal

When the user corrects the agent ("no, never use X", "always do Y"), the hook notices it with a cheap pattern list and
the one Stop ask (item 03) adds a short clause asking the agent to save it as a `rule`. The agent writes the rule; Cairn
never writes it on its own.

## Out of scope

Saving the correction automatically (no LLM; a raw prompt is not a rule). Preference detection. Multi-language patterns.

## Decisions it relies on

D-LLM (host writes the rule), D-TOK.

## Design

- **Detection** in `UserPromptSubmit` (`Hook::handle`, `crates/client/src/hook/mod.rs`): a pure function
  `is_correction(line) -> bool` in `crates/core/src/process/distill.rs` (next to `has_typed_line`, so it is unit-tested
  in core). A prompt line (outside code fences, after `is_boilerplate`) whose lower-case text starts with one of:
  `no,` `no ` `don't ` `do not ` `never ` `always ` `stop ` `please don't ` `please never ` `please always ` `wrong` `that's wrong` `i told you`.
- State: `SessionState.correction: Option<String>` keeps the first such line, clipped to 100 chars (`event::clip`).
  Not set for automated/subagent sessions (item 12).
- **Ask**: `stop_nudge` fires when files changed (item 03) **or** a correction was seen. When a correction is present, the
  hook appends (or, without file changes, sends alone) the clause from new constant `DEFAULT_CORRECTION_NUDGE` in
  `crates/core/src/instructions.rs`: `You were corrected ("{correction}"): save it as a rule if it holds beyond this task.`
  (≤ 90 bytes + the quote). Still once per session in total.
- The clause is built-in only in 0.1.9 (no new editable field, no migration); `off` (item 03) disables it too.

## Allowed files

- `crates/core/src/process/distill.rs` (new pure fn + unit tests only)
- `crates/core/src/instructions.rs`
- `crates/client/src/hook/mod.rs`, `crates/client/src/hook/state.rs`
- tests: `crates/server/tests/hook.rs`

## Acceptance checks

1. `is_correction` is true for "No, never use npm here", "always run bun test first", "don't touch the migrations"; false for "can you always-on the logs?", "I never saw this error before?" (lower-cased start is `i `), "Decision: use bun".
2. A session with a correction and no edits gets one ask containing the quoted correction (≤ 100 chars) and the word `rule`.
3. A session with a correction and edits gets one ask with both parts; total ≤ 330 bytes.
4. Corrections inside fenced code are ignored.
5. Subagent/automated sessions record no correction.

## Tests to write first

- `distill.rs::correction_patterns` (1, 4)
- `hook.rs::correction_alone_triggers_rule_ask` (2)
- `hook.rs::correction_with_edits_single_ask` (3)
- `hook.rs::subagent_corrections_ignored` (5)

## Data-safety notes

No storage. The correction text stays in the local hook state file (already private, swept after 7 days, `state.rs::STALE`).

## Token cost

+~25 tokens plus the quote, only in sessions with a correction; still one ask per session.

## Risks

False triggers ("no worries") cost one clause once; the agent may decline. Patterns are start-of-line only to limit this.

## Open questions

- Non-blocking: item 03 says the ask is for sessions that changed files; this item also asks after a correction without
  edits (advisory A7). Default: **yes**, a correction alone triggers the one ask.
