---
id: CM-DOC-TECHNICAL-BENCHMARKS
title: "Benchmarks"
status: accepted
description: "Read Cairn’s fixture-based retrieval, capture and latency checks."
---

# Benchmarks

Fixture-based retrieval metrics, CI quality gates, weak-query constraints, and recall latency benchmarks.

The benchmark runner `scripts/bench.py` evaluates real server retrieval accuracy, weak-query resistance, latency, and context token overhead. Stored fixture records live in `bench/fixtures/` and `bench/aliases-0.1.9.md`.

## CI gates

Continuous integration enforces minimum accuracy thresholds and zero weak-query leakage across test fixtures:

| Benchmark run | Fixture corpus | Distractors | Minimum hit@5 floor | Maximum weak-query hits |
| --- | --- | --- | --- | --- |
| **v1 Benchmark** | 70 core memories | 0 | **≥ 0.88** (link ratio ≥ 0.60) | **0 / 5** |
| **v2 Benchmark** | 43 test questions | 2,000 distractors | **≥ 0.65** | **0 / 14** |

### Measured release performance (0.1.9 Shipped)

Real benchmark results from `bench/after-0.1.9-aliases-v1-shipped.json` and `bench/after-0.1.9-aliases-v2-shipped.json`:

| Metric | v1 Shipped Result | v2 Shipped Result (2,000 distractors) |
| --- | --- | --- |
| **hit@5** | **0.920** | **0.674** |
| **MRR@10** | **0.860** | **0.663** |
| **precision@5** | **0.510** | **0.460** |
| **Off-topic queries empty** | 5 / 5 (100%) | 14 / 14 (100%) |
| **Search reply size** | ~133 tokens | ~133 tokens |
| **Session brief overhead** | ~1,160 tokens | ~1,162 tokens |

*Cross-encoder experiment: A local reranker raised v2 hit@5 from 0.674 to 0.744, but admitted 8 off-topic weak queries (only 6/14 empty) and sharply increased p95 latency. It was not shipped.*

## Latency

The opt-in latency suite measures p50, p95, and maximum latency across 20 distinct requests per recall path against a disposable database:

```sh
python3 scripts/bench.py --questions questions-v2.json --distractors 2000 \
  --check-recall-latency --out /tmp/cairn-recall-latency.json
```

### Measured latency budgets (ARM64 macOS, MiniLM, 2,072 memories)

| Path | HTTP p95 (Budget &lt; 100 ms) | Hook end-to-end p95 (Budget &lt; 500 ms) | Status |
| --- | --- | --- | --- |
| **Session Brief** | 12.1 ms | 128.0 ms | Pass |
| **File Topics** | 18.4 ms | 164.2 ms | Pass |
| **Command Warning** | 22.0 ms | 195.0 ms | Pass |
| **Failed Command** | 31.5 ms | 240.1 ms | Pass |
| **Prompt Recall** | 54.3 ms | 442.8 ms | Pass |

See [Automatic recall latency details](../../../../bench/README.md#automatic-recall-latency).
