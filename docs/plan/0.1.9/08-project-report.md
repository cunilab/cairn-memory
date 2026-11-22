---
id: CM-PLAN-019-08
title: "08 Per-project web report"
status: frozen
---

# 08 Per-project web report

## Goal

The project page shows the last sessions of the project: when, branch, changed files, notes and digest the distill
wrote, saves the agent made, and, when nothing was saved, why (for example "no edits", "no decision lines, no test runs").
You see at once that a project writes nothing, without reading code.

## Out of scope

Raw capture listing (privacy, volume). A dashboard-wide project filter (advisory J2 beyond this page). Any action buttons.

## Decisions it relies on

D-DATA, D-LLM, automation-first web (reports + essential tasks only).

## Design

- Core: `distill()` (`crates/core/src/process/distill.rs:64`) returns `why_none: Vec<&'static str>` when it produced no
  notes, from fixed codes: `no_edits`, `no_typed_lines`, `no_test_runs`, `no_fixed_failures`, `all_duplicates`
  (set by the store when every save was a duplicate), `automated` (item 12).
- Migration `0010_session_outcome.sql` (additive, nullable; existing rows stay NULL = "not recorded"):
  `ALTER TABLE sessions ADD COLUMN distilled_at timestamptz, ADD COLUMN notes_written integer, ADD COLUMN digest_written boolean, ADD COLUMN handoff_outcome text CHECK (handoff_outcome IN ('written','kept','skipped','none')), ADD COLUMN why_none text[], ADD COLUMN files_changed integer;`
  Backfill: none needed beyond NULL; the report shows "before 0.1.9" for NULL rows.
- Store: `Store::distill_session` (`crates/store/src/capture.rs:279`) writes these columns at the end of each distill.
  New `Store::project_sessions(person_id, project_id, days) -> Vec<SessionReport>` in `crates/store/src/project.rs`
  (member check via `is_member`, l.321): last 50 sessions with `started_at`, `ended_at`, branch (`context->>'branch'`),
  `automated`, the outcome columns, `captures` (count of retained observations), and `agent_saves` = memories
  with `origin='agent'`, same owner and project, `created_at` between session start and end (or last capture). The UI
  labels it "saves during the session".
- Route: `GET /cairn-memory/projects/{project_id}/sessions?days=30` (`crates/server/src/routes/project.rs`, registered
  in `routes/mod.rs`), web auth like `branches` (l.288). `days` ∈ {7, 30, 90}. ts-rs type `SessionReport`.
- Web: `apps/web/app/projects/_components/project-sessions.tsx` (new), a table in `ProjectDetail`
  (`project-detail.tsx`) under the branches block. Columns: Started, Branch, Files, Notes, Digest, Handoff, Agent saves,
  Why nothing. A header line: "N sessions, M with saves". Uses the existing `DataTable`, `Time`, `Badge` components.

## Allowed files

- `migrations/0010_session_outcome.sql` (new), `crates/store/src/lib.rs`
- `crates/core/src/process/distill.rs`
- `crates/store/src/capture.rs`, `crates/store/src/project.rs`
- `crates/server/src/routes/project.rs`, `crates/server/src/routes/mod.rs`
- `apps/web/app/projects/_components/project-sessions.tsx` (new), `apps/web/app/projects/_components/project-detail.tsx`, `apps/web/lib/generated/*` (regenerated)
- tests: `crates/store/tests/processing.rs`, `crates/server/tests/project_model.rs`, `crates/store/tests/migrate.rs`, a web unit test next to the component, `scripts/smoke_web.sh`

## Acceptance checks

1. A session with no edits and only chat distills with `notes_written=0`, `why_none` containing `no_edits`.
2. A session with edits and a passing `cargo test` records `files_changed`, `digest_written=true`, notes ≥ 1.
3. `GET .../sessions` for a non-member returns 404 (same code as other project routes); for a member returns rows newest first, ≤ 50.
4. An agent `memory_save` during a session is counted in that session's `agent_saves`.
5. Migrating a 0.1.8 database keeps every `sessions` row; their new columns are NULL; the page shows "before 0.1.9".
6. The project page renders the table and the "N sessions, M with saves" line (web unit test + `smoke_web.sh` step).

## Tests to write first

- `distill.rs::why_none_codes` (1)
- `processing.rs::distill_records_session_outcome` (1, 2)
- `project_model.rs::project_sessions_member_only` (3, 4)
- `migrate.rs::session_outcome_columns_nullable` (5)
- `project-sessions.test.tsx::renders_summary_line` (6)

## Data-safety notes

Additive nullable columns. Reads only; no web action changes data.

## Token cost

0 for agents (web only).

## Risks

The time-window attribution of agent saves can count a save from a parallel session in the same project; the label says "during".

## Open questions

None.
