---
id: CM-DOC-TECHNICAL-BENCHMARKS
title: "Benchmarks"
status: accepted
description: "Read Cairn’s fixture-based retrieval, capture and latency checks."
---

# Benchmarks

The [benchmark README](../../../../bench/README.md) documents running the real server with the fixture corpus, versioned questions and optional distractors. Keep the marker-bearing [alias set](../../../../bench/aliases-0.1.9.md) in place. `scripts/bench_write.py` checks capture, digests and handoffs.

## CI gates

The v2 run adds 2,000 distractors and gates on hit@5 of at least 0.65 with zero weak-query hits. The v1 run gates on hit@5 of at least 0.88 and link ratio of at least 0.60. Use the companion holdout fixtures to check behavior on separate questions. CI also uploads the result JSON.

[Stored v1 results for 0.1.9](../../../../bench/after-0.1.9-aliases-v1-shipped.json) · [Stored v2 results for 0.1.9](../../../../bench/after-0.1.9-aliases-v2-shipped.json).

## Latency

The optional local recall check drives HTTP and real hooks against a disposable PostgreSQL database. Each path receives three warm-up requests and 20 timed requests with distinct sessions. The script reports p50, p95 and maximum latency. Its p95 limits are under 100 ms for HTTP and under 500 ms for hooks. Timing depends on the fixture, local hardware, model, Docker and process scheduling. Use the [benchmark README](../../../../bench/README.md#automatic-recall-latency) for the invocation and recorded hardware.
