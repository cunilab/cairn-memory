---
id: CM-EXP-001
title: "Embedding model comparison on bench v2 (experiment, not for merge)"
status: accepted
---

# Embedding model comparison on bench v2 (experiment, not for merge)

Outcome: the recommendation below was applied on `0.1.6` (MiniLM kept, weak path removed, tau_s = 0.43; [ADR 0015](../adr/0015-0.1.6-memory-model-and-agent-surface.md)). The experiment scaffolding stayed on the experiment branch.

Branch `Andres-Holivin/embed-exp`, from `0.1.6` @ f5d36f2. Question: is all-MiniLM-L6-v2 the bottleneck of step-5 retrieval
(bench v2: 43 questions + 2000 distractors; hit@5 0.628, MRR 0.493, paraphrase 3/17, weak zero-hit 7/14; holdout 0.10)?

**Answer: no.** Seven models (4 small, 2 base, 1 int8 base) all land at the same place once the gate is tuned: main hit@5 0.65-0.70
with zero weak-query leaks. The largest single improvement comes from the gate policy on MiniLM alone: remove the weak path (tau_w)
and set tau_s ≈ 0.43. See the recommendation at the end.

## Method

- Real pipeline: the real `cairn-server` (Postgres + pgvector, hybrid lexical/entity/vector, gate, rank, pack), bench v2 main
  questions + 2000 seeded distractors (`scripts/bench.py --questions questions-v2.json --distractors 2000`). Only the embedder changes
  (env: pooling, query/document prefix, dimension). Default tau (0.40/0.25) on MiniLM reproduces the documented 0.628 / 0.493 / 0.185 / 3/17 / 7/14.
- Calibration: `bench.py --experiment-out` sweeps (tau_s, tau_w) over the **main set only** through `/search` with a debug-only override
  (grid 0.025 coarse; 0.005 steps around each model's weak-query cosine ceiling; tau_w from off to tau_s-0.25). Objective J = hit@5 + weak zero-hit rate;
  ties: MRR, then higher tau_s. The spec 4.3 rule (midpoint of max weak cosine and 4th paraphrase cosine, tau_w = tau_s-0.15) is reported too.
- Holdout: once per model, at that model's J-best taus (`--holdout-taus auto`). It was never read before. Exception: bge-small, e5-small, gte-small holdout ran at the
  *coarse*-grid J-best (one run each); their main numbers below come from the later fine-grid sweep (no second holdout look).
  bge-small: holdout at (0.65,0.65), fine J-best is (0.634, off).
- Gate-free check (`scripts/embed_exp/offline.py`, onnxruntime): vector-only ranking over the same 2043 memories, no gate, no lexical channel.
- Latency/RSS (`scripts/embed_exp/lat.py`): onnxruntime CPU, 2 intra-op threads, median of 20 (single) / 5-8 (batch). **Caveats: Apple-silicon macOS, not an x86 2-vCPU
  server, with other jobs running (load avg 12-20); expect a 1.5-3x slower server. Peak RSS is the whole Python process (varies +-30% run to run, MiniLM 246-316 MB); server RSS was not measured.**

## Models

| model | dims | pooling / prefixes | ONNX size fp32 (int8) | license (HF card, not re-verified) |
|---|---|---|---|---|
| all-MiniLM-L6-v2 (current) | 384 | mean | 90 MB (23) | Apache-2.0 |
| bge-small-en-v1.5 | 384 | CLS, query "Represent this sentence for searching relevant passages: " | 133 MB (34) | MIT |
| e5-small-v2 | 384 | mean, "query: " / "passage: " | 133 MB | MIT |
| gte-small | 384 | mean | 133 MB | MIT |
| bge-base-en-v1.5 | 768 | CLS, same query prefix | 436 MB (110) | MIT |
| nomic-embed-text-v1.5 | 768 | mean, "search_query: " / "search_document: " | 547 MB (137) | Apache-2.0 |
| gte-base (offline + latency only) | 768 | mean | 436 MB | MIT |

All load in the existing fastembed `UserDefinedEmbeddingModel` path (ONNX + tokenizer.json) unchanged; max length 256 for all.

## Results, main set (n=43, 14 weak), per model at its best operating point

| model | tau_s / tau_w | hit@5 | MRR@10 | prec@5 | paraphrase | weak zero-hit | leak cards | holdout hit@5 / MRR (once) |
|---|---|---|---|---|---|---|---|---|
| MiniLM, shipped taus | 0.40 / 0.25 | 0.628 | 0.493 | 0.185 | 3/17 | 7/14 | 24 | 0.10 / 0.114 (spec §13) |
| **MiniLM, J-best** | 0.454 / off | 0.674 | 0.616 | 0.500 | 4/17 | 14/14 | 0 | 0.20 / 0.150 |
| **MiniLM, tau_s 0.429 / off (recommended)** | 0.429 / off | 0.651 | 0.605 | 0.429 | 4/17 | 14/14 | 0 | not run (same family as above) |
| bge-small | 0.634 / off | 0.698 | 0.572 | 0.230 | 6/17 | 14/14 | 0 | 0.30 / 0.213 (at 0.65/0.65) |
| e5-small | 0.834 / off | 0.651 | 0.605 | 0.373 | 3/17 | 14/14 | 0 | 0.40 / 0.192 (at 0.825/off) |
| gte-small | 0.838 / off | 0.698 | 0.628 | 0.352 | 5/17 | 14/14 | 0 | 0.70 / 0.383 (at 0.825/off) |
| gte-small, recall-leaning | 0.823 / off | 0.837 | 0.669 | 0.243 | 10/17 | 12/14 | 6 | (band 0.015 wide, see below) |
| bge-base | 0.558 / off | 0.698 | 0.556 | 0.269 | 6/17 | 14/14 | 0 | 0.20 / 0.125 |
| bge-base int8 | 0.600 / 0.600 | 0.651 | 0.580 | 0.429 | 3/17 | 14/14 | 0 | 0.00 / 0.014 |
| nomic-embed-text-v1.5 | 0.672 / off | 0.651 | 0.605 | 0.478 | 3/17 | 14/14 | 0 | 0.00 / 0.000 |

Spec 4.3 rule (a gap between max weak cosine and the 4th paraphrase cosine exists for MiniLM and the five models listed): the point it picks (tau_s = midpoint, tau_w = tau_s-0.15) leaks: MiniLM 0.431/0.281 -> 9/14 zero-hit, 16 cards;
bge-small, e5-small, gte-small, bge-base and nomic: 4-5/14, 39-46 cards, hit@5 0.54-0.61 (bge-base int8 not checked). The tau_s half is fine; the tau_w half is what breaks it.

Holdout weak queries: every model, at every tau, returns the same 5 cards on 1 of 5 weak queries. That one passes by lexical coverage or entity, not by cosine, so no model or tau fixes it.

### Gate-free vector ranking (offline, whole store, same 43 questions)

| model | hit@5 | hit@10 | MRR@10 | paraphrase hit@5 | paraphrase median rank | best gold cos above every weak-query cos |
|---|---|---|---|---|---|---|
| MiniLM | 0.767 | 0.860 | 0.732 | 7/17 | 8 | 0.651 (para: 0.235) |
| MiniLM int8 | 0.767 | 0.791 | 0.717 | 7/17 | 13 | 0.674 |
| bge-small | 0.767 | 0.814 | 0.705 | 7/17 | 8 | 0.791 (0.529) |
| e5-small | 0.744 | 0.791 | 0.721 | 7/17 | 6 | 0.674 |
| gte-small | 0.744 | 0.767 | 0.697 | 6/17 | 15 | 0.674 |
| gte-base | 0.791 | 0.837 | 0.728 | 8/17 | 6 | 0.791 |
| bge-base | 0.814 | 0.884 | 0.794 | 9/17 | 5 | 0.767 (0.471) |
| bge-base int8 | 0.860 | 0.860 | 0.799 | 11/17 | 3 | 0.814 (0.588) |
| nomic v1.5 | 0.860 | 0.860 | 0.752 | 11/17 | 4 | 0.721 |

Base models do rank a little better (+0.05-0.09 hit@5, paraphrase 11/17 vs 7/17), but that does not survive the gate: the end-to-end table shows no gain,
because the gate must separate a paraphrase (cosine 0.35-0.5 on MiniLM) from a weak query that shares a domain word (up to 0.40 on MiniLM), and that overlap shows in every model
(the gate-free column above: even the best model has the best gold cosine above every weak-query cosine for only 0.81 of questions, and for 0.59 of paraphrases). Compressed-range models (e5, gte: all cosines 0.75-0.9) make it worse for calibration: gte-small's usable band is
tau_s 0.823-0.838 (0.015 wide); MiniLM's zero-leak plateau is 0.409-0.464 (0.055 wide).

## Cost per model (2 threads, single text = ~94-token memory, query = short)

| model | query ms | doc ms | batch 8 docs ms | batch 32 docs ms | peak RSS MB | file MB |
|---|---|---|---|---|---|---|
| MiniLM | 2.7-3.9 | 11.7-18.2 | 98-134 | 464-569 | 246-316 | 90 |
| MiniLM int8 | 1.7 | 9.1 | 84 | 355 | 162 | 23 |
| bge-small | 6.2 | 24.8 | 220 | 1045 | 323 | 133 |
| bge-small int8 | 4.2 | 21.4 | 174 | 728 | 158 | 34 |
| e5-small | 7.8 | 33.1 | 232 | 992 | 309 | 133 |
| gte-small | 5.7 | 27.4 | 238 | 974 | 348 | 133 |
| gte-base | 20.9 | 99.8 | 975 | 4060 | 825 | 436 |
| bge-base | 22.1 | 98.8 | 991 | 4090 | 674 | 436 |
| bge-base int8 | 14.4 | 76.7 | 668 | 2773 | 333 | 110 |
| nomic v1.5 | 28.9 | 138.7 | 1333 | 5802 | 916 | 547 |
| nomic v1.5 int8 | 18.1 | 88.9 | 857 | 3753 | 719 | 137 |

Every model fits the owner's budget (peak RSS <= 1.2 GB, single embed < 300 ms on this machine, even at 2-3x slower). Nomic fp32 is marginal at 916 MB. No model was rejected on resources; they were rejected on lack of benefit.
Indexing the 2070-memory bench store took about 450-500 s for every small and int8 model (3 threads, shared loaded machine; timing is not comparable to production).

## MiniLM alone: lower tau_s or raise tau_w? (main set)

| change | hit@5 | MRR | prec@5 | para | weak zero | leak cards |
|---|---|---|---|---|---|---|
| shipped 0.40 / 0.25 | 0.628 | 0.493 | 0.185 | 3/17 | 7/14 | 24 |
| lower tau_s only, 0.30 / 0.25 | 0.698 | 0.521 | 0.181 | 6/17 | 4/14 | 32 |
| raise tau_w only, 0.40 / 0.30 | 0.674 | 0.570 | 0.248 | 4/17 | 9/14 | 15 |
| raise tau_w only, 0.40 / 0.35 | 0.651 | 0.596 | 0.306 | 4/17 | 9/14 | 7 |
| raise tau_w to tau_s (weak path off), 0.40 | 0.651 | 0.602 | 0.379 | 4/17 | 13/14 | 1 |
| weak path off, tau_s 0.429 | 0.651 | 0.605 | 0.429 | 4/17 | 14/14 | 0 |
| weak path off, tau_s 0.454 (J-best) | 0.674 | 0.616 | 0.500 | 4/17 | 14/14 | 0 |
| weak path off, tau_s 0.30 (recall-leaning) | 0.767 | 0.633 | 0.226 | 8/17 | 6/14 | 24 |

Raising tau_w dominates lowering tau_s; lowering tau_s alone buys recall by leaking more. Switching the weak path off dominates the shipped config on every metric. Above 0.464 hit@5 starts falling (0.475: 0.628).
Trade-off if the owner prefers recall: tau_s 0.30-0.35 with the weak path off gives hit@5 0.70-0.77, MRR 0.64-0.66, but 6-8/14 weak queries leak (10-24 cards).

## Recommendation

**Keep all-MiniLM-L6-v2. Remove the weak path of the gate, set tau_s = 0.43.** No model change, no schema change, no new download.

Expected on bench v2 main (measured at 0.429): hit@5 0.651 (was 0.628), MRR 0.605 (0.493), precision 0.429 (0.185), weak zero-hit 14/14 (7/14), leak cards 0 (24). Holdout: 0.20 / 0.15 at 0.454 (was 0.10 / 0.114): about the same, so
do not expect the holdout gap (6 of 10 gold memories have cosine below 0.36) to close with any tau or any model tested; that needs a different retrieval idea, not a threshold.

Why not a bigger model: bge-base / nomic / int8 gain +0 to +0.05 hit@5 at zero leak on main (0.651-0.698), lose on MRR or holdout, cost 4-8x the latency and 2-3x the RSS, and need `vector(768)`. gte-small is the one outlier (holdout 0.70, main
recall-leaning 0.837), but it only works inside a 0.015-wide cosine band on 10 holdout questions; one more distractor shifts it. Not a safe config.

Exact changes (applied on `0.1.6`; the experiment patch on the experiment branch is test scaffolding and was not merged):

1. `crates/core/src/retrieve/gate.rs`: delete `COSINE_WEAK`; `COSINE_STRONG = 0.43`;
   `keep` = `g.entity_hit || g.coverage >= COVERAGE_MIN || g.cosine.is_some_and(|c| c >= COSINE_STRONG)`. Update `gate_keeps_entity_coverage_or_cosine` and `gate_bounds_are_exact_edges` (drop the weak-path assertions).
2. `scripts/bench.py` CI gates in `.github/workflows/ci.yml` (`bench` job): v2 `--max-weak-hits 28` -> about 3; keep `--min-hit5 0.58`.
3. Spec `docs/specs/0.1.6-step5-retrieval.md` §4.2/§4.3: drop the weak path and the `tau_w = tau_s - 0.15` clause; record tau_s = 0.43 (rule midpoint was 0.431).
4. Re-run `cargo test --workspace` (server tests with the real model, e.g. `off_topic_queries_stay_empty_among_400_distractors`, `semantic.rs`) and bench v1 (`--min-hit5 0.90 --max-weak-hits 0`): v1 has not been run with the weak path off. Not verified here.

If the owner still wants a bigger model later, the schema change is `vector(384)` -> `vector(768)` in `migrations/0001_foundation.sql:118` and `0003_retrieval.sql:9`, `EMBEDDING_DIMENSIONS` in `crates/core/src/process/embed.rs`, a per-model manifest (pooling, query/document prefix, dimension, hashes) replacing the embedded `minilm.json`, and re-embedding (data is wipeable).

## Limits of this experiment

- n is small: 43 main (17 paraphrase), 10 holdout. A 0.02 hit@5 difference is one question.
- Calibration is on the same 43 questions it is scored on (the tau is picked from them), so main numbers are optimistic; the holdout is the only out-of-sample check, and it is too small to rank models.
- Latency/RSS are from macOS Apple silicon under load, not the 2-core/4 GB target; server RSS (ORT arena + Postgres) was not measured.
- Query prefixes were not tuned (bge-small without the instruction gives the same vector-only hit@5 0.767 offline). Max length 256 for all.
- Relative gates (margin to the k-th cosine, query-level z-score) were not tested; they are the obvious next lever because absolute cosine overlaps for all models.
- Experiment scaffolding in this branch (global-atomic tau override behind `CAIRN_DEBUG_SCORES`, `CAIRN_EMBED_*` env, runtime `vector(N)` replace in the migration runner, `bench.py --experiment-out`) is for reproduction only.
- Data: the small result files are in [`embed-exp/`](./embed-exp/) (holdout runs, latency, offline vector-only); the full sweeps (`bench/embed-exp/*.json`) and scripts (`scripts/embed_exp/`) stay on the experiment branch `Andres-Holivin/embed-exp`.
