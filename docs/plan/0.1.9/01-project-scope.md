---
id: CM-PLAN-019-01
title: "01 Distilled notes take project scope, low rank"
status: frozen
---

# 01 Distilled notes take project scope, low rank

## Goal

A gotcha, fact or typed note that the hook distills in a project checkout is saved with project scope, so the project page,
project counts and teammates see it. Every automatic note starts at rank `candidate` (ranked lower, never in the brief)
and becomes `normal` once it is used or confirmed.

## Out of scope

Handoff scope (stays personal; item 05). Rescoping existing personal rows (see open question). Decay/TTL changes.

## Decisions it relies on

D-DATA, D-RANK, D-LLM (rule-based distill unchanged apart from scope).

## Design

- Today `crates/core/src/process/distill.rs` hard-codes `scope: Some(Scope::Personal)` in `gotchas` (l.212),
  `test_fact` (l.260) and `typed_notes` (l.523). `Store::distill_session` (`crates/store/src/capture.rs:279`) knows the
  session's `project_id`.
- Change: `distill()` leaves `scope: None` on notes; `distill_session` sets `Scope::Project` when `session_project.is_some()`,
  else `Scope::Personal`, before `save_as(..., Origin::System, ...)`. Handoff keeps `Scope::Personal`.
- Rank. Migration `0007_memory_rank.sql`:
  `ALTER TABLE memories ADD COLUMN rank text NOT NULL DEFAULT 'normal' CHECK (rank IN ('candidate','normal'));`
  Backfill: the `DEFAULT` sets every existing row to `normal` (existing memories keep today's behaviour); the migration
  also runs `UPDATE memories SET rank = 'normal' WHERE rank IS DISTINCT FROM 'normal' AND origin = 'agent';` as an
  explicit no-op guard. Registered in `crates/store/src/lib.rs` migration list.
- `Memory` (`crates/core/src/memory/model.rs`) gains `rank: Rank` (`Candidate | Normal`, serde lower-case, ts export).
- Insert: `Origin::System` notes (not handoffs) are inserted with `rank = 'candidate'`; agent saves stay `normal`.
- Promotion to `normal` (one `UPDATE ... SET rank='normal' WHERE rank='candidate'`):
  - `Store::mark_fetched` (`crates/store/src/memory.rs:247`, used by `memory_get`/search fetch);
  - `Store::feedback` with `helpful` (`memory.rs:420`);
  - an exact duplicate save of the same identity (the `ON CONFLICT ... DO NOTHING` path in `memory.rs:650`), and a
    consolidate supersede where the newer side is a re-statement (item 11 adds proof count to this path).
- Ranking: `crates/core/src/retrieve/rank.rs::adjust` adds `-0.10` for `Rank::Candidate` inside the existing ±0.30 cap.
- Brief: `Store::brief` (`crates/store/src/retrieve.rs:564`) excludes `rank='candidate'` rows.
- Counts: `project_counts` (`crates/store/src/project.rs:426`) counts candidates too (they are project knowledge) and
  returns a separate `candidates` number for the web.

## Allowed files

- `migrations/0007_memory_rank.sql` (new), `crates/store/src/lib.rs`
- `crates/core/src/process/distill.rs`, `crates/core/src/memory/model.rs`, `crates/core/src/retrieve/rank.rs`
- `crates/store/src/capture.rs`, `crates/store/src/memory.rs`, `crates/store/src/retrieve.rs`, `crates/store/src/project.rs`
- `apps/web/lib/generated/*` (regenerated only)
- tests: `crates/store/tests/processing.rs`, `crates/store/tests/migrate.rs`, `crates/core/src/retrieve/rank.rs` (unit)

## Acceptance checks

1. A distilled gotcha in a session with a project is stored `scope='project'`, `rank='candidate'`, `origin='system'`.
2. The same session without a project stores `scope='personal'`, `rank='candidate'`.
3. Handoffs are unchanged: `scope='personal'`, `rank='normal'`.
4. `memory_get` on a candidate makes it `normal`; a `helpful` vote does too.
5. A candidate never appears in `/session/brief`.
6. `adjust` for an otherwise identical candidate is exactly 0.10 lower (within the cap).
7. Migrating a 0.1.8 database with rows keeps all rows, all `rank='normal'`.
8. `project_counts` includes the project-scope candidate.

## Tests to write first

- `processing.rs::distilled_notes_take_project_scope_and_candidate_rank` (1, 2, 3)
- `processing.rs::using_a_candidate_promotes_it` (4)
- `retrieve.rs::brief_skips_candidates` (5) in `crates/store/tests/retrieve.rs`
- `rank.rs::candidate_ranks_lower` (6)
- `migrate.rs::rank_backfill_keeps_rows_normal` (7)

## Data-safety notes

Additive column with default; no row changes scope. Promotion only moves `candidate → normal`, never back.

## Token cost

0 added. Brief gets smaller or equal (candidates excluded).

## Risks

Rule-based mistakes become team-visible; mitigated by low rank, 180-day system TTL (`kind.rs::SYSTEM_TTL_SECONDS`) and supersede.

## Open questions

- Non-blocking: should existing personal system notes that carry a `project_id` be moved to project scope? Default:
  **no** (it changes who can see existing data); they stay as they are.
