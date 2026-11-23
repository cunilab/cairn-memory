---
id: CM-BENCH-019-RERANK
title: "0.1.9 cross-encoder experiment"
status: frozen-output
---

# Cross-encoder experiment, 0.1.9 (item 10)

Date: 2026-11-23. Result: **ship rule not met; the reranker is not merged.** This commit holds only this document, the two
JSON result files and the `p50_ms`/`p95_ms` fields in `scripts/bench.py` (a measurement fix kept in both outcomes). The
experiment code (diff saved outside the repo by the worker) was reverted before the commit.

## What was built and run

- Runtime: `fastembed` 7.7.1 `TextRerank` loaded from user-defined files (no download inside the server), behind a process-global
  `OnceLock` in `crates/store/src/retrieve.rs`, registered from `crates/server/src/worker.rs` when `CAIRN_RERANK_MODEL_DIR` was set.
- Two-key gate as in the spec: a candidate the gate rejects (`gate::keep`) with cosine in `[0.30, 0.43)` (at most 20, best cosine
  first, only those the request filter allows) is scored with (query, `title + " " + summary`); a score at or above the threshold passes.
  Candidates above the gate are untouched. Unset env: the old path, byte for byte.
- Models, in the spec's order (pinned revision, sha256 of `onnx/model.onnx` matches the Hugging Face LFS hash):

| model | revision | `model.onnx` size | sha256 (`model.onnx`) |
|---|---|---|---|
| `jinaai/jina-reranker-v1-turbo-en` (smaller; the one benched in full) | `b8c14f4e723d9e0aab4732a7b7b93741eeeb77c2` | 151,296,975 B (144.3 MiB) + tokenizer 1.9 MB | `c1296c66c119de645fa9cdee536d8637740efe85224cfa270281e50f213aa565` |
| `BAAI/bge-reranker-base` (`BGERerankerBase`) | `2cfc18c9415c912f9d8155881c133215df768a70` | 1,112,459,588 B (1.04 GiB) + tokenizer 17.1 MB | `15b9a8c3da82eddf263df571281166e00e9308fe19d077084b642ebfcaf06d2b` |

- Bench: `scripts/bench.py`, v2 (2,000 distractors, 43 main questions, 14 weak) and v1, MiniLM from `~/.cache/cairn-bench/minilm`,
  scripts at this commit plus a local-only `--rerank-model-dir` flag. Scores are raw logits (all negative).
- Item 09 shipped `ALIASES_IN_SEARCH = false` (`bench/aliases-0.1.9.md`), so the **primary** result is the shipped config (aliases
  stored, not searched). Once more with `ALIASES_IN_SEARCH = true` in a local build only (never committed): second table.

## Threshold (chosen on v2 main questions only)

Rule written before the sweep: pick the threshold with the best v2 main hit@5; ties go to the higher threshold. Weak queries and the
holdout were not read for the choice. Jina turbo, `memory_search` v2 main hit@5 / MRR@10 (each a full bench run, aliases off):

| threshold (`RERANK_KEEP`) | hit@5 | MRR@10 |
|---|---|---|
| no reranker | 0.6744 | 0.6628 |
| -100 (every band candidate passes) | 0.7442 | 0.7209 |
| -4.6 | 0.7442 | 0.7209 |
| -4.1 | 0.7209 | 0.7093 |
| **-3.5 (chosen)** | **0.7442** | 0.7171 |
| -3.0 | 0.6977 | 0.6860 |

Every value above -2.9 admits none of the six gold notes that fall in the band, so it equals "no reranker" (not run separately).
The chosen `RERANK_KEEP` is **-3.5**.

## Result, shipped config (aliases off), jina turbo at -3.5

Files: `bench/after-0.1.9-rerank-v2.json`, `bench/after-0.1.9-rerank-v1.json` (rerank on). The "off" column is the same build with
the env unset, run the same day.

| measure | rerank off | rerank on |
|---|---|---|
| v2 hit@5 | 0.6744 | **0.7442** |
| v2 MRR@10 | 0.6628 | 0.7171 |
| v2 precision@5 (gold cards / cards) | 0.4533 (34 / 75) | 0.2587 (37 / 143) |
| v2 weak, empty | 14 / 14 | **6 / 14** |
| holdout hit@5 / MRR@10 (v2 run) | 0.20 / 0.12 | 0.30 / 0.2411 |
| holdout weak, empty (v2 run) | 5 / 5 | **3 / 5** |
| v1 hit@5 | 0.92 | 0.96 |
| v1 weak, empty (5) | 5 / 5 | 4 / 5 |
| v1 holdout hit@5 / weak empty | 0.30 / 5 of 5 | 0.60 / 5 of 5 |
| p50 / p95 search latency, v2 (ms) | 118.5 / 132.5 | 243.8 / 511.3 |
| p50 / p95 search latency, v2, repeat run (ms) | 113.7 / 131.5 | 239.2 / 497.9 |
| p50 / p95 search latency, v1 (ms) | 96.9 / 113.9 | 121.8 / 199.7 |

Latency is a **local measurement** (no CI runner was available): this Mac, ONNX on CPU with 1 intra-op thread, 43 `memory_search`
calls at limit 5 per run (nearest-rank percentiles, MCP round trip included), so p95 is the 41st of 43 sorted calls.

## Result with aliases searched (local build only)

| run (v2 unless noted) | hit@5 | MRR@10 | weak empty | holdout hit@5 | holdout weak empty | p50 / p95 ms |
|---|---|---|---|---|---|---|
| aliases on, rerank off | 0.6977 | 0.669 | 9 / 14 | 0.30 | 5 / 5 | 116.7 / 136.5 |
| aliases on, rerank on (-3.5) | 0.7674 | 0.7163 | 5 / 14 | 0.30 | 2 / 5 | 459.3 / 530.9 |
| v1, aliases on, rerank on (-3.5) | 0.96 | 0.96 | 4 / 5 | 0.70 (rerank off: 0.50, `aliases-0.1.9.md`) | 5 / 5 | 199.4 / 278.8 |

The aliases-on, rerank-off row reproduces the `bench/aliases-0.1.9.md` numbers (0.6977, 9 / 14).

## Why it cannot pass (scores from the logged runs)

The reranker scores of the candidates in the band, v2 (a run with every band candidate allowed through, scores logged; the weak
numbers here explain the failure, they were not used to pick the threshold):

| | jina turbo | bge-reranker-base |
|---|---|---|
| gold notes in the band, scores | -4.50 .. -2.92 (6) | -10.11 .. -5.62 (6) |
| best non-gold distractor on a main question | -1.34 | -2.49 |
| best candidate on a weak query | -1.52 | -4.88 |

On both models every weak-query candidate in the band scores above the gold notes (jina) or above all but the best gold
(bge: gold max -5.62 < weak max -4.88). So any threshold low enough to rescue a gold paraphrase also lets a weak query return a
card. The generated distractors share engineering vocabulary with the queries; the cross-encoder ranks them above the paraphrase
gold. The bigger model is worse, so "try the other model" does not change the outcome. `BGERerankerBase` was only logged (score
analysis above), not run through the full table.

## Ship decision

Rule (spec, all required): v2 hit@5 > 0.72; v2 weak 14/14 empty; holdout weak 5/5 empty; v1 hit@5 >= 0.88; p95 search latency
increase <= 60 ms on the CI runner.

| condition | result | met |
|---|---|---|
| v2 hit@5 > 0.72 | 0.7442 | yes |
| v2 weak 14/14 empty | 6 / 14 | **no** |
| holdout weak 5/5 empty | 3 / 5 | **no** |
| v1 hit@5 >= 0.88 | 0.96 | yes |
| p95 increase <= 60 ms (local, not the CI runner) | +378.8 ms (132.5 to 511.3); repeat +366.4 ms | **no** |

**Not shipped.** The reranker code is not merged, the model is not added to the downloader or CI, and the v2 floor stays 0.65.
The only 0.1.9 bench change kept is the `p50_ms` / `p95_ms` output in `scripts/bench.py`.

## Acceptance check 1 (feature off changes nothing)

With the reranker code present and `CAIRN_RERANK_MODEL_DIR` unset, `recall`, `zero_hit`, `recall_holdout`, `zero_hit_holdout` and
`per_question_memory_search` were compared field by field with `bench/after-0.1.9-aliases-v2-shipped.json` and
`bench/after-0.1.9-aliases-v1-shipped.json`: all equal for v2 and v1. The unit test `gate.rs::rerank_only_between_bounds`
and `semantic.rs::rerank_off_changes_nothing` passed in the experiment tree and went away with the reverted code.
