---
id: CM-PLAN-019-10
title: "10 Cross-encoder experiment"
status: frozen
---

# 10 Cross-encoder experiment

## Goal

Measure whether a small local cross-encoder, used as a second judge for candidates just under the cosine gate, lifts
bench v2 hit@5 above 0.72 while every weak query still returns nothing. Ship it only if it does.

## Out of scope

Any LLM. Replacing the embedder. Re-ranking candidates already above the gate. Shipping when the bar is not met.

## Decisions it relies on

D-LLM (a cross-encoder model file is allowed only if the bench proves it), D-TOK, D-SHIP.

## Design

- Runtime: `fastembed` is already a server dependency (`crates/server/Cargo.toml`, `=7.7.1`); use its `TextRerank`
  with one small reranker it supports (try `JINARerankerV1TurboEn`, then `BGERerankerBase`; pick the smaller that
  passes). No new crate.
- Model download: `scripts/download_embedding_model.py` gains an optional `--reranker` mode (pinned revision + sha256,
  like the MiniLM files). Env `CAIRN_RERANK_MODEL_DIR`; unset = feature off (today's behaviour).
- Two-key gate (advisory B3): in the search path (`crates/store/src/retrieve.rs::search`, l.329, after
  `load_candidates`), candidates that fail `gate::keep` (`crates/core/src/retrieve/gate.rs:30`) but have cosine in
  `[0.30, 0.43)` — at most 20, best cosine first — are scored by the reranker with (query, title + summary). Those with
  score ≥ `RERANK_KEEP` pass the gate. Candidates above the gate are untouched.
- Threshold: chosen on bench v2 **main** questions only, by sweep; reported on holdout without tuning.
- Experiment output: `bench/rerank-0.1.9.md` (new): model, size on disk, threshold, v2 hit@5 / MRR / weak, holdout
  hit@5 / weak, v1 hit@5, p50/p95 search latency with and without, from `scripts/bench.py` runs saved as
  `bench/after-0.1.9-rerank-v2.json` and `-v1.json`.
- **Ship rule** (all required): v2 hit@5 > 0.72; v2 weak 14/14 empty; holdout weak 5/5 empty; v1 hit@5 ≥ 0.88;
  p95 search latency increase ≤ 60 ms on the CI runner. If met: the feature is on when the model dir is set, the CI
  `Bench` job downloads the model and raises `--min-hit5` for v2 to (result − 0.025), and release notes say so.
  If not met: the experiment doc is committed, the reranker code is **not merged**, and the floor stays 0.65.
- Runs after item 09 so the measurement includes aliases (both on and off reported).

## Allowed files

- `crates/store/src/retrieve.rs`, `crates/core/src/retrieve/gate.rs` (constant + pure keep-with-rerank fn), `crates/server/src/worker.rs` (where the fastembed model is built)
- `scripts/download_embedding_model.py`, `scripts/test_download_embedding_model.py`, `scripts/bench.py` (flag only)
- `bench/rerank-0.1.9.md` (new), `bench/after-0.1.9-rerank-v1.json`, `bench/after-0.1.9-rerank-v2.json` (new)
- `.github/workflows/ci.yml` (only if the ship rule passes)
- tests: `crates/server/tests/semantic.rs`, unit tests in `gate.rs`

## Acceptance checks

1. With `CAIRN_RERANK_MODEL_DIR` unset, every existing bench number is identical to 0.1.9 before this item.
2. `gate.rs` unit test: a candidate at cosine 0.44 passes without the reranker being called; at 0.29 it is never sent.
3. `bench/rerank-0.1.9.md` contains every number listed in Design, and the ship decision with the rule quoted.
4. If shipped: CI downloads the pinned model with a sha256 check, and the v2 floor is raised as stated.
5. If not shipped: no reranker code in the merged diff (only the doc and JSON results).

## Tests to write first

- `gate.rs::rerank_only_between_bounds` (2)
- `semantic.rs::rerank_off_changes_nothing` (1)
- `test_download_embedding_model.py::reranker_checksum_enforced` (4, only if shipped)

## Data-safety notes

No schema change, no stored data.

## Token cost

0 more per card; it can return a card where today none is returned (that card's normal cost).

## Risks

Latency on CPU; the 20-candidate cap and the ship rule bound it. Overfitting the threshold to 43 questions; the holdout is reported untouched.

## Open questions

None.
