---
id: CM-PLAN-019-12
title: "12 Subagents and automated sessions"
status: frozen
---

# 12 Subagents and automated sessions

## Goal

Subagent sessions and automated sessions (any orchestrator, scheduled run or worker) never get the Stop ask (item 03),
never track corrections (item 06) and never overwrite the person's handoff (item 05). They still capture, and their
distilled notes are still saved as candidates.

## Out of scope

Agent identity on memories (advisory I1). Orca-specific detection. Hiding automated sessions from reports (item 08 shows them, labelled).

## Decisions it relies on

D-ANY (not Orca-specific), D-TOK, D-RANK.

## Design

- **Detection** (client, `crates/client/src/hook/mod.rs`, once at `SessionStart`, stored in `SessionState` as
  `automated: bool`; re-checked on every event because some hosts skip `SessionStart` for subagents):
  1. env `CAIRN_SESSION_ORIGIN=automation` (documented; any orchestrator sets it; Orca users put it in their worker env);
  2. the hook payload says it runs in a subagent: a non-empty `agent_id` or `agent_type` field (Claude Code
     subagent hook input), read with `Payload::text` (`hook/event.rs:45`);
  3. the event is `SubagentStop` (handled like `Stop` for capture, but never asks).
- **Effects**:
  - `stop_nudge` (`mod.rs`, after the `stop_hook_active` check) returns `None` when `automated`.
  - Correction tracking (item 06) is skipped.
  - Captures carry `context.origin = "automation"`; the server stores it in `sessions.context` (jsonb, no migration)
    via `upsert_session` (`crates/store/src/capture.rs:540`).
  - `Store::distill_session` (`capture.rs:279`) skips the handoff write when the session context has
    `origin = "automation"`. Notes are still saved (rank `candidate`, item 01). The audit `session_distilled` event
    records `automated: true`.
- Docs: one paragraph in `docs/user-guide.md` naming `CAIRN_SESSION_ORIGIN`.

## Allowed files

- `crates/client/src/hook/mod.rs`, `crates/client/src/hook/state.rs`, `crates/client/src/hook/event.rs`
- `crates/core/src/capture/observation.rs` (only if `origin` needs a typed slot in `WorkContext`; prefer the jsonb context)
- `crates/store/src/capture.rs`
- `docs/user-guide.md` (one paragraph)
- tests: `crates/server/tests/hook.rs`, `crates/store/tests/processing.rs`

## Acceptance checks

1. With `CAIRN_SESSION_ORIGIN=automation`, a session that edits files gets no Stop ask.
2. A Stop payload with `agent_id` set gets no Stop ask, even if the parent session id has edits.
3. An automated session's distill writes notes but no handoff; the previous handoff for the checkout is unchanged (same row, same `updated_at`).
4. A normal session in the same checkout afterwards still gets its ask and writes its handoff.
5. `session_distilled` audit for the automated session has `automated: true`.

## Tests to write first

- `hook.rs::automation_env_suppresses_the_ask` (1)
- `hook.rs::subagent_payload_suppresses_the_ask` (2)
- `processing.rs::automated_session_never_overwrites_handoff` (3, 5)
- `hook.rs::normal_session_after_automation_still_asks` (4)

## Data-safety notes

Removes a cause of handoff loss. No schema change.

## Token cost

Negative: no ask in automated/subagent sessions.

## Risks

If a host does not mark subagents, a subagent may still get the ask; the env variable remains the reliable switch.

## Open questions

- Non-blocking: the exact Claude Code subagent payload fields (`agent_id`, `agent_type`) must be confirmed against the
  current hook docs by the implementer; if they differ, use the documented names and note it in the report.
