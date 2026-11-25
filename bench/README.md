# Cairn benchmarks

`scripts/bench.py` drives the real server and measures retrieval, weak-query results, latency and agent-context size. Fixtures are under `bench/fixtures/`. The script reads the sample aliases in `bench/aliases-0.1.9.md`; keep the markers and file in place. `scripts/bench_write.py` exercises capture, handoffs, digests and the save ask.

CI runs v2 with 2,000 distractors, a hit@5 floor of 0.65 and no weak-query hits. It also runs v1 with a hit@5 floor of 0.88. These are fixture measurements, not a guarantee of performance on other data.

## Model and gate

The server uses local all-MiniLM-L6-v2 embeddings when configured. Comparisons with larger models did not show a reliable end-to-end gain at the same weak-query constraint. The relevance gate accepts an entity hit, sufficient lexical coverage or cosine of at least 0.43. Lowering the cosine gate recovered some paraphrases but admitted off-topic results because their scores overlap. The 0.65 floor leaves a small margin below the measured v2 result; a small fixture makes individual questions consequential.

## Aliases and reranking

Aliases are stored and embedded, but alias vector search is disabled: enabling it increased weak-query hits in the benchmark. A local cross-encoder trial increased v2 hit@5 from 0.6744 to 0.7442 but only 6 of 14 weak queries stayed empty, and local p95 latency rose substantially. It is not shipped. These measurements were made on a small test set and local hardware; they do not establish production latency.


## Automatic recall latency

Run the latency gate locally with Docker, the pinned MiniLM model and a quiet development machine.

```sh
python3 scripts/bench.py --questions questions-v2.json --distractors 2000 \
  --check-recall-latency --out /tmp/cairn-recall-latency.json
```

The check seeds a disposable PostgreSQL database, waits for embedding and linking, and runs the brief, file topic, command warning, failed-command and prompt paths through loopback HTTP and the real hook binary. Each path has three warm-up requests and 20 measured requests in distinct sessions. The JSON includes p50, p95 and maximum latency. The command fails if a path misses recall, if any HTTP p95 reaches 100 ms, or if any hook p95 reaches 500 ms. The gate is opt-in because process scheduling, Docker and embedding performance vary across CI runners.

A local ARM64 macOS run with MiniLM and 2,072 memories measured HTTP p95 at 12.1–63.9 ms and hook p95 at 128.0–462.2 ms. Prompt recall measured 54.3 ms over HTTP and 442.8 ms end to end. Every sample met its budget in this run.
