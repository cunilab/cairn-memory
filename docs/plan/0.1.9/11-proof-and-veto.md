---
id: CM-PLAN-019-11
title: "11 Proof count and rare-token veto"
status: frozen
---

# 11 Proof count and rare-token veto

## Goal

When the same knowledge is saved again, Cairn counts it ("seen 3×") instead of silently merging, and ranks it slightly
higher. Two near-identical texts that differ in a version, number or file name are linked, never merged.

## Out of scope

Contradiction detection (D7). Changing the 0.92 / 0.75 thresholds (`crates/core/src/process/consolidate.rs`).

## Decisions it relies on

D-DATA (prevents silent loss by wrong merges), D-LLM, D-RANK.

## Design

- Migration `0008_proof_count.sql`: `ALTER TABLE memories ADD COLUMN proof_count integer NOT NULL DEFAULT 1 CHECK (proof_count >= 1);`
  Backfill (idempotent, sets from history; never lowers):
  `UPDATE memories m SET proof_count = 1 + s.n FROM (SELECT <newer side>, count(*) n FROM links WHERE relation='supersedes' AND origin='auto' GROUP BY 1) s WHERE m.memory_id = s.<newer side> AND m.proof_count < 1 + s.n;`
  The implementer confirms which column (`source_id` or `target_id`) is the newer side from `supersede()` in
  `crates/store/src/consolidate.rs` and states it in the report.
- **Increment**:
  - exact duplicate save (identity conflict path, `crates/store/src/memory.rs:629-680`): `proof_count = proof_count + 1`,
    and `rank='normal'` (item 01);
  - consolidate supersede (`Store::consolidate_memory`, `consolidate.rs:59`): newer `proof_count += older.proof_count`.
- **Rank**: `rank.rs::adjust` adds `0.02 * (min(proof_count, 4) - 1)` (max +0.06) inside the ±0.30 cap.
- **Card**: `Memory` gains `proof_count: u32`; the search card (`Cards`, built by `pack()` in
  `crates/core/src/retrieve/pack.rs:110`) gets `seen: N`, serialized only when N ≥ 2 (`skip_serializing_if`, like `is_zero`).
- **Rare-token veto**: new pure `fn rare_tokens_differ(a: &str, b: &str) -> bool` in `consolidate.rs` (core). Tokens are
  title + summary words that contain a digit (`20`, `v3`, `0.1.9`), a `/`, or a `.ext` file suffix, or are wrapped in
  backticks. True when the two sets differ. `guard()` (core `consolidate.rs:40`) takes the two texts and returns
  `Band::Relate` instead of `Supersede` when they differ. The caller in `crates/store/src/consolidate.rs` passes them.

## Allowed files

- `migrations/0008_proof_count.sql` (new), `crates/store/src/lib.rs`
- `crates/core/src/process/consolidate.rs`, `crates/core/src/memory/model.rs`, `crates/core/src/retrieve/rank.rs`, `crates/core/src/retrieve/pack.rs`
- `crates/store/src/consolidate.rs`, `crates/store/src/memory.rs`
- `apps/web/lib/generated/*` (regenerated only)
- tests: core unit tests, `crates/store/tests/processing.rs`, `crates/store/tests/migrate.rs`

## Acceptance checks

1. Saving the same gotcha 3 times leaves one row with `proof_count = 3`; its search card has `seen: 3`.
2. "Use node 20 for the desktop app" then "Use node 22 for the desktop app" (cosine ≥ 0.92): both stay `active`, linked `relates`.
3. Two texts differing only in wording, same tokens, cosine ≥ 0.92: supersede as today; newer `proof_count` = sum.
4. `adjust` grows by 0.02 per extra proof up to +0.06, still capped at ±0.30.
5. Backfill on a 0.1.8 database: no row lost, every `proof_count ≥ 1`; running the backfill twice changes nothing.
6. A card with `proof_count = 1` is byte-identical to 0.1.8's.

## Tests to write first

- `consolidate.rs::rare_tokens_veto_supersede` (2) and `same_tokens_still_supersede` (3)
- `rank.rs::proof_count_boost_capped` (4)
- `processing.rs::duplicate_saves_count_proofs` (1)
- `migrate.rs::proof_backfill_idempotent` (5)
- `pack.rs::card_unchanged_for_single_proof` (6)

## Data-safety notes

Additive column. The veto turns some merges into links, so fewer rows become `superseded`; nothing is deleted.

## Token cost

`seen` adds ~3 tokens to a card, only when N ≥ 2.

## Risks

More near-duplicates stay active; the diversity cap in `pack.rs` limits crowding.

## Open questions

None.
