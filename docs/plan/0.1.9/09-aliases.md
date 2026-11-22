---
id: CM-PLAN-019-09
title: "09 Aliases, backfill, regenerated bench"
status: frozen
---

# 09 Aliases, backfill, regenerated bench

## Goal

A memory can carry up to 5 short "ways to ask" (aliases) written by the host agent at save time. Each alias has its own
vector; search scores a memory by the best of its main vector and its alias vectors. Existing memories get derived
aliases (title, gotcha symptom) by backfill, with no LLM. The bench is regenerated with realistic aliases to measure it honestly.

## Out of scope

Aliases written by Cairn with a model. Changing the gate constants (`crates/core/src/retrieve/gate.rs`). Per-field
embeddings beyond the two derived aliases.

## Decisions it relies on

D-LLM (agent writes aliases; derived ones are copies of existing text), D-DATA, D-TOK, D-TOOLS.

## Design

- Migration `0009_memory_aliases.sql`:
  `CREATE TABLE memory_aliases (memory_id text NOT NULL REFERENCES memories ON DELETE CASCADE, ord smallint NOT NULL, source text NOT NULL CHECK (source IN ('agent','derived','learned')), text text NOT NULL CHECK (char_length(text) BETWEEN 1 AND 120), embedding vector(384), embedding_source text, PRIMARY KEY (memory_id, ord));`
  `CREATE INDEX memory_aliases_hnsw ON memory_aliases USING hnsw (embedding vector_cosine_ops);`
  Backfill (idempotent, `ON CONFLICT DO NOTHING`): for every non-forgotten memory, `ord 100` = `title`;
  for gotchas with a non-empty `fields->>'symptom'`, `ord 101` = that symptom clipped to 120 chars. `source='derived'`,
  `embedding` NULL.
- `source='learned'` is allowed by the CHECK for aliases an admin applies from Fixes F11 (item 15). This item writes
  no `learned` rows; they count toward the 5-alias limit and are searched like the others.
- Embedding: the existing embed path (`crates/store/src/embed.rs`: `pending_embeddings`, `enqueue_missing_embeddings`,
  `upsert_embedding`) also selects alias rows with `embedding IS NULL` and fills them with the same MiniLM model. No new
  job kind (avoids a `jobs_kind_check` change).
- Save: `memory_save` (`crates/client/src/tools.rs:45`) and each `items[]` entry (item 03) accept
  `aliases: array of string, maxItems 5, each ≤ 120 chars`. `SaveInput` (`crates/core/src/memory/model.rs:61`) gains
  `aliases: Vec<String>` (validated in `crates/core/src/memory/validate.rs`; not part of `identity_hash`). Stored as
  `source='agent'`, `ord 0..4`; a supersede copies nothing (the new memory brings its own).
- Search: `Store::vector_candidates` (`crates/store/src/retrieve.rs:225`) unions alias hits with memory hits and keeps
  per memory the max cosine. The gate and ranking see that cosine; nothing else changes.
- Description sentence for agents (≤ 25 tokens): `aliases: up to 5 short questions a future agent might ask that this answers.`
  The item-03 ask is not changed.
- **Bench**: `bench/fixtures/corpus.json` gains `aliases` per gold memory, written once offline by a host agent that sees
  only the memory text, never the questions (prompt and run date recorded in `bench/aliases-0.1.9.md`). Distractors get
  aliases from the same offline procedure on a 200-note sample, the rest only derived ones. `scripts/bench.py` loads them
  and adds `--no-aliases` for the A/B. Results saved as `bench/after-0.1.9-aliases-v2.json` and `-v1.json`.
- **Ship rule**: aliases ship enabled only if v2 weak stays 14/14 empty, holdout weak 5/5, and v2 hit@5 ≥ 0.674 (no
  regression). Otherwise the table and backfill still ship (additive) but `vector_candidates` ignores alias rows behind
  a constant `ALIASES_IN_SEARCH = false`, and the bench doc says so.

## Allowed files

- `migrations/0009_memory_aliases.sql` (new), `crates/store/src/lib.rs`
- `crates/core/src/memory/model.rs`, `crates/core/src/memory/validate.rs`
- `crates/store/src/memory.rs`, `crates/store/src/embed.rs`, `crates/store/src/retrieve.rs`
- `crates/client/src/tools.rs`, `crates/server/src/routes/memory.rs`
- `scripts/bench.py`, `bench/fixtures/corpus.json`, `bench/aliases-0.1.9.md` (new), `bench/after-0.1.9-aliases-v1.json` and `-v2.json` (new)
- `apps/web/lib/generated/*` (regenerated only)
- tests: `crates/store/tests/migrate.rs`, `crates/store/tests/retrieve.rs`, `crates/server/tests/semantic.rs`

## Acceptance checks

1. Backfill on a 0.1.8 database: one `derived` alias per non-forgotten memory (+1 per gotcha with a symptom); memory rows unchanged; second run inserts 0.
2. After the embed pass, no alias row has `embedding IS NULL` (for non-expired memories).
3. A save with 6 aliases, or one of 121 chars, is rejected; with 5 valid ones stores 5 `agent` rows.
4. A query whose only close vector is an alias returns that memory (semantic test with a fixed fixture).
5. Forgetting a memory leaves no alias rows reachable in search (forgotten filter applies).
6. Bench A/B recorded; ship rule applied and stated in `bench/aliases-0.1.9.md`.
7. `memory_save` schema growth from aliases ≤ 200 bytes.

## Tests to write first

- `migrate.rs::alias_backfill_idempotent` (1)
- `retrieve.rs::aliases_embedded_and_searched` (2, 4)
- `memory.rs` (store test) `alias_limits` (3)
- `retrieve.rs::forgotten_aliases_not_returned` (5)

## Data-safety notes

New table only; memory rows are never rewritten. `ON DELETE CASCADE` follows the existing hard-delete sweep of expired memories.

## Token cost

Schema ~45 tokens per session. Output: agent spends ~20–60 tokens per note on aliases. Search replies unchanged in size.

## Risks

Aliases can pull weak queries over the 0.43 gate (`gate.rs::COSINE_STRONG`); the ship rule guards it. Fake gains if bench aliases see the questions; the procedure forbids it.

## Open questions

- Non-blocking: who generates the offline bench aliases. Default: one worker run of the host agent with the recorded
  prompt, output committed and reviewed like code.
