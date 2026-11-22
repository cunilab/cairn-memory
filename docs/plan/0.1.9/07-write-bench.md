---
id: CM-PLAN-019-07
title: "07 Write-side bench in CI"
status: frozen
---

# 07 Write-side bench in CI

## Goal

CI fails if a scripted session no longer produces the memories, digest, handoff and single ask that items 01–06 and 12
promise. Today the bench only measures search; this protects writing from regressing.

## Out of scope

Measuring agent compliance (no LLM in CI). Recall numbers (the existing `bench.py`).

## Decisions it relies on

D-LLM (scripted, no model), D-TOK, D-ANY.

## Design

- New `scripts/bench_write.py` (stdlib + the same server start helpers as `scripts/bench.py`: `free_port`, `run`).
  Starts the built server on a temp schema, registers a device, runs `cairn hook <Event>` with JSON payloads from a
  fixture, then reads results through the HTTP API.
- Fixture `bench/fixtures/write-sessions.json`, four scripted sessions in one temp git repo plus one non-git folder:
  1. **work** (Claude payloads): start; prompt `Decision: use bun for tests because ...`; prompt `No, never edit migrations by hand`;
     prompt `we should maybe tidy the roadmap`; prompt `next: wire the web report`; edit `src/a.ts`; `bun test` fails
     with an error; edit `src/a.ts`; `bun test` passes; Bash `rm src/old.ts`; Stop; Stop; SessionEnd.
  2. **chat** in the same checkout: start; prompt `what does this function do?`; Stop; SessionEnd.
  3. **subagent**: same as work but Stop payloads carry `agent_id` and env `CAIRN_SESSION_ORIGIN=automation`.
  4. **no-git**: `cairn init` in a plain folder, then an edit inside it and one in `/tmp`, Stop, SessionEnd.
- Waits for jobs (`/admin/jobs` pending 0) and asserts (each a named check, printed pass/fail, exit 1 on any fail):
  - W1 work: ≥ 1 decision note and ≥ 1 gotcha, all `scope=project`, `rank=candidate`.
  - W2 work: one digest naming `src/a.ts`, `src/old.ts` and `bun test pass`.
  - W3 work: handoff `next_steps == ["next: wire the web report"]`; `files` include `src/old.ts`.
  - W4 work: first Stop output is the ask, mentions `rule` and the correction; second Stop prints nothing.
  - W5 chat: no ask; the work handoff is unchanged (`updated_at` equal).
  - W6 subagent: no ask; handoff unchanged; notes saved as candidates.
  - W7 no-git: captures attached to the project; summary says `(1 outside the project)`.
  - W8 tokens: total hook-injected bytes for session 1 ≤ the 0.1.9 baseline (1,621 B) + 250 B.
- Writes `--out` JSON (counts + per-check result), like `bench.py`.
- CI: a step in the existing `Bench` job of `.github/workflows/ci.yml` after the v1/v2 runs:
  `python3 scripts/bench_write.py --skip-build --out "$RUNNER_TEMP/bench-write.json"`; the artifact upload includes it.

## Allowed files

- `scripts/bench_write.py` (new), `bench/fixtures/write-sessions.json` (new), `bench/write-0.1.9.json` (new, first result)
- `.github/workflows/ci.yml` (one step + artifact path)

## Acceptance checks

1. On the finished 0.1.9 branch, all W1–W8 pass and the script exits 0.
2. Reverting item 05's marker change (`should ` back in the list) makes W3 fail (checked once by hand, output pasted in the PR).
3. Removing the item-12 check makes W6 fail (same).
4. The script runs in under 90 s in CI and needs no network beyond the model download already in the job.
5. The `Bench` job uploads `bench-write.json`.

## Tests to write first

The fixture and `bench_write.py` are written first and run against the branch **before** items 01–06/12 land: record the
failing W-checks in the PR as the baseline (expected: W1–W7 fail, W8 passes). Then each item turns its checks green.

## Data-safety notes

Temp schema only, dropped by the test harness the same way `bench.py` does; never touches a real database.

## Token cost

0 at runtime. W8 guards the budget.

## Risks

Flaky timing on job completion; the script polls `/admin/jobs` with a 60 s timeout, not sleeps on fixed delays.

## Open questions

None.
