---
id: CM-PLAN-019-14
title: "14 Fixes: detectors"
status: frozen
---

# 14 Fixes: detectors

## Goal

Once a night, a server pass with no LLM checks data Cairn already has and writes **fix rows** (problem, evidence,
exact action). It never changes memories, devices, instructions or files. It also marks Agent fixes done when their
proof appears.

## Out of scope

Apply, Ignore, the web page (15). Device execution and `cairn fixes` (16). Not built: D5, D9, handoff/correction
detectors, D6 "heavily changed".

## Decisions it relies on

D-LLM, D-DATA, D-TOK. User: choice B; detectors F1–F9 ON at launch; F10–F12 switch on automatically when their data
window exists (still need Apply); Ignore reappears when the problem comes back; never inject fixes into sessions.

## Design

- Pure predicates in `crates/core/src/fixes/` (new): one fn per detector over plain rows, returning
  `(fingerprint, subject, evidence, action)`. Fingerprint = detector + subject id + hash of stable inputs (no counts, no times).
- Store queries in `crates/store/src/fixes.rs`. Nightly run once per server day (last run time in `server_state`),
  triggered from the existing sweep loop in `crates/server/src/worker.rs`. Never through the `jobs` table.
- Detectors (types: Server = Cairn changes its data on Apply; Computer = the device does it, item 16; Agent = Copy
  prompt; Report = no Apply):
  - **F1 Agent**: one project, 7 days, ≥ 3 non-automated sessions that changed a file or ran a command, `notes_written = 0`
    (digest excluded), 0 agent saves. Counts only sessions started ≥ 7 days after the first `sessions.nudge_asked IS NOT NULL` row.
  - **F2 Agent**: memory `stale`, `stale_cause='feedback'` ≥ 7 days, fetched after going stale. Skip global/system.
  - **F3 Agent**: `stale_cause='missing'` ≥ 7 days and a later `checked_at` still missing. Skip superseded.
  - **F4 Computer** `suggest_setup`: active device, `last_hook_at` > 7 days old (not null), `last_seen_at` < 2 days.
    `hook_error_class` (item 13) adds a reason when present.
  - **F5 Computer** `run_update`: `version_older(client_version, VERSION)` (`crates/core/src/account/device.rs:128`);
    unparsable is not old. Target version stored in `action`.
  - **F6 Computer** `write_project_file`: one person, 7 days, ≥ 2 sessions on one non-`local:` repository with no
    `project_id`, exactly one project has it. Two matches → Report "pick one".
  - **F7 Server** `split_merge`: an `origin = auto` `supersedes` link where item 11's `rare_tokens_differ` is true.
  - **F8 Report**: semantic search off, active memories without embedding > 1 h, or foreign embedding model.
  - **F9 Server** `redact_secret`: `redact_sensitive_content` (`crates/core/src/capture/redact.rs:82`) would change a
    memory's title/summary/reason/fields. Evidence holds the pattern class and field name only, never the text.
  - **F10 Server** `nudge_off` (on after 14 days of `nudge_asked`): one project, 14 days, ask shown ≥ 10 times, ≤ 1 followed by a save.
  - **F11 Server** `add_alias` (on after 14 days of `lookup_log` and item 09 live): in one session search A got 0
    cards, a later search returned M and M was opened; cosine(A, M) ≥ 0.43; same A→M pair in ≥ 2 sessions. Max 5 aliases per memory.
  - **F12 Server** `fade` (on once a memory > 90 days exists): created > 90 days, not fetched 90 days, kind not
    decision/rule/procedure/gotcha/preference/handoff, project had ≥ 20 searches in 90 days.
- Aliases and fades never target `scope = global` or `origin = system`.
- **Lifecycle per night**: a match with an open fix bumps `count`/`last_seen_at`; no match sets `quiet_seen_at`. An
  open (pending) fix whose problem is gone becomes `resolved`. An `ignored` fix reopens as a new pending row only
  after one quiet night, a later match, and `ignored_until` (14 days; F5 until VERSION changes; F10 30; F12 90).
- **Proof** (also nightly): Agent fix `waiting` → `done` when exactly one memory with `supersedes = M`, new identity,
  same project, created after `waiting_since` exists (F1: any agent-origin save in the project after `waiting_since`).
  Feedback or forget never count. `waiting` > 14 days → `expired`; `queued` > 7 days → `expired`.
- `faded_at` rank penalty in `crates/core/src/retrieve/rank.rs`; a fetch clears `faded_at` (`crates/store/src/retrieve.rs`).

## Allowed files

- `crates/core/src/fixes/` (new), `crates/core/src/lib.rs` (module line)
- `crates/store/src/fixes.rs`, `crates/server/src/worker.rs`
- `crates/core/src/retrieve/rank.rs` (faded penalty only), `crates/store/src/retrieve.rs` (clear on fetch only)
- tests: `crates/core/src/fixes/` unit tests, `crates/store/tests/fixes.rs` (new)

## Acceptance checks

1. Per detector, a fixture at the threshold fires and one just below does not.
2. A young single-member store fixture opens 0 fixes.
3. Superseded, global and system memories never fire.
4. Two runs the same night create no duplicate; a second night only bumps `count`.
5. Ignored fix stays hidden while the problem stays; reopens after a quiet night plus cooldown.
6. The pass writes only `fixes` rows (row counts of every other table unchanged).
7. F10–F12 produce nothing before their switch-on condition; F1 ignores sessions before the 7-day gate.
8. A faded memory ranks below an equal unfaded one; a fetch clears it.
9. No evidence JSON contains memory text, query text, commands or paths (scan test).

## Tests to write first

- `core fixes::tests::*_threshold` (1), `store/tests/fixes.rs::young_store_quiet` (2), `::guards` (3),
  `::idempotent_night` (4), `::ignore_reappears` (5), `::only_fixes_written` (6), `::switch_on` (7),
  `::evidence_has_no_text` (9); `retrieve.rs` (store) `::faded_ranks_lower` (8)

## Data-safety notes

Read-only except the `fixes` table and clearing `faded_at` on fetch.

## Token cost

0: nothing reaches agent context.

## Risks

False alarms on small stores (thresholds and the young-store test guard it). 

## Open questions

None.
