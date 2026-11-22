# Bench v2 miss analysis for 0.1.9

Question: can retrieval quality go up enough to raise the CI Bench floor from hit@5 0.60 to 0.68 (target 0.72)?
**Answer: not generally, on MiniLM.** The floor can move to 0.65. Reasons below.

Data: `bench/before-0.1.9-v2.json` (the 0.1.8 code, bench v2, 2,000 distractors; same as the 0.1.7 numbers: hit@5 0.674,
MRR 0.663, precision@5 0.460, weak 14/14 empty, holdout 0.20) and `bench/scores-0.1.9-before.json`
(`bench.py --dump-scores`, now also the five highest candidate cosines per query and whether each is a gold memory).

## Where the 14 misses are

hit@5 is 29 of 43. By kind: direct 19/19, near-duplicate 5/5, paraphrase 4/17, paraphrase_stem 1/2. All 14 misses are
paraphrases written to share no word with the gold memory, so only the semantic channel can find them. Every gold the
gate keeps is already in the top 5 (first gold rank is 1 or 2 in all 29 hits), so **ranking is not the problem; the
gate and the embedding are.**

| query | gold cosine | gold rank by cosine | top candidate cosine | why it misses |
|---|---|---|---|---|
| q18 | 0.333 | 2 | 0.344 (distractor) | below the 0.43 gate; a distractor is closer |
| q20 (stem) | 0.338 | 1 | 0.338 | below the gate; coverage 0.17 |
| q33 | 0.367 | 1 | 0.367 | below the gate |
| q37 | 0.295 | 2 | 0.303 (distractor) | low cosine, distractor closer |
| q35 | 0.397 | 8 | 0.489 | **collision**: distractors are closer and pass the gate, so 5 wrong cards come back |
| q36 | 0.340 | 20 | 0.432 | collision (1 wrong card) |
| q43 | 0.346 | 14 | 0.407 | below the gate, distractors closer |
| q39 | 0.297 | 17 | 0.354 | low cosine |
| q42 | 0.196 | 23 | 0.268 | the paraphrase is too far for MiniLM |
| q32, q34, q38, q40, q41 | not in the 128 fused candidates | | 0.26 to 0.50 | gold cosine is below every candidate that made the pool; q32 and q40 return 5 passing distractor cards |

Counting the 14: 10 return nothing because the best candidate (gold or not) is under the 0.43 gate (q18, q20, q33,
q34, q37, q38, q39, q41, q42, q43); 4 return wrong cards because a distractor above the gate crowds the gold out
(q32 5 cards, q35 5, q36 1, q40 5).
No miss is caused by the lexical channel, the kind and freshness adjustments (at most plus or minus 30 % of a score
that is only compared after the gate), the candidate pool size (a gold with cosine above 0.43 would be at vector rank 1
to 5 and is always in the 128), or the diversity cap.

## Why the gate cannot move

Weak queries (off-topic, 14) reach a top cosine of 0.405 (w13), 0.391 (w2), 0.372 (w11), 0.370 (w9). The recoverable golds
sit at 0.333 to 0.398. A gold-versus-weak sweep over every threshold from 0.25 to 0.70 on the cosine alone gives at
most **28 hits with zero weak leaks, at 0.405**; production already gets 29 (one extra through the word match of q17).
No threshold buys a hit.

Relative signals do not separate them either. Gap between the best and the second candidate: q20 0.027, q33 0.036
(recoverable) against w4 0.027, w2 0.037, w6 0.061, w8 0.089 (weak). The weak queries have the same shape as a
true paraphrase hit.

## What was tried and measured (no code kept)

- **Query side, stopwords stripped before embedding and search** (live stack, bench v2 questions through
  `/search` with `debug`): max weak cosine 0.391, zero-leak ceiling 28 of 43, the same as the baseline. Gold
  cosines move both ways (q15 0.46 to 0.43, q20 0.338 to 0.371, q39 0.297 to 0.242). Not kept.
- **Gate by prominence** (best candidate far above the next ones): see the gap numbers above; it admits weak queries
  before it admits q20 or q33.
- **Candidate pool, RRF mix, adjust weights**: cannot change a gold that fails the gate; a larger pool only adds
  candidates the gate drops.
- **Embedding model**: already measured and rejected in `docs/experiments/embedding-model-comparison.md` (seven models
  land at 0.65 to 0.70 with zero weak leaks; a gate-free MiniLM ranking is 0.77 but then every weak query returns
  cards).

## Distractor collisions (a property of the bench)

The distractors are generated engineering notes in the same project, so a paraphrase of a rare topic can sit near
an unrelated note (q35 0.489, q40 0.504, q32 0.458, q14 0.512 above its gold at 0.489). Those notes pass the gate, which
is correct behaviour for the gate and the reason precision@5 stays at 0.46.

## Holdout (reported, not tuned on)

Holdout hit@5 0.20 (2 of 10; h06 0.491, h09 0.517 and h10 0.443 pass the gate, h10 is not top 5), weak 5/5 empty.
Same picture: the misses have gold cosine 0.25 to 0.37 with a distractor closer. The holdout weak queries reach 0.351.

## Decision

- No retrieval change in 0.1.9: nothing general moves hit@5 without leaking a weak query, and weak-query silence is
  the property users notice (a wrong card costs more than a missing one).
- The CI floor goes from 0.60 to **0.65**: the run is deterministic (0.6744 reproduced exactly twice), so the margin is
  0.024. 0.68 is above the result (0.674) and would fail today; one question is worth 0.023, so a gate needs about two questions of margin to survive noise.
- What would move the number: a better embedder or a re-ranker (rejected for size and latency in 0.1.6), or a labelled
  paraphrase set to learn a gate from. Both are beyond a 0.1.x patch.
