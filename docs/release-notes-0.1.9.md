---
id: CM-REL-005
title: "Cairn Memory 0.1.9 release notes"
status: draft
---

# Cairn Memory 0.1.9 release notes

A retrieval-quality investigation, a higher CI bench floor, and two review fixes. No schema change, no new tools,
**no retrieval change**.

## What changed for users

- **`cairn init` no longer silently takes ownership of a lost project.** When `.cairn/project.json` names an id the
  server has never heard of (a wiped or new server), whoever registers it first becomes its owner. `cairn init` now
  refuses with a warning (HTTP 409 from the server) and tells you to run `cairn init --yes` if that is intended.
  Joining a project the server knows is unchanged. The server needs `confirm_register: true` in the init request to
  restore an unknown id; clients older than 0.1.9 that hit this case now get the 409 message.
- **Fewer hook requests.** A session asks the server for topic notes at most 16 times (found or not; a path is still
  asked once). Before, a session that opened hundreds of files with no notes made one request per file.

## Retrieval and the bench floor

We looked for a general retrieval fix to raise the CI `Bench` floor from hit@5 0.60 to 0.68 (target 0.72) and found
none that keeps weak queries silent. Details: [`bench/analysis-0.1.9.md`](../bench/analysis-0.1.9.md).

- All 14 v2 misses are paraphrases with no shared word. Every gold the gate keeps is already in the top 5; 10 misses
  have a best candidate under the 0.43 cosine gate, 4 are crowded out by distractors that pass it.
- The off-topic (weak) queries reach cosine 0.405, above the recoverable golds (0.33 to 0.40), so no threshold, gap or
  query rewrite gains a hit without returning cards for an off-topic question. Stripping stopwords from the query was
  measured and gave nothing.
- The floor is therefore **0.65** (was 0.60): the run is deterministic at 0.674, a margin of 0.024. 0.68 would fail today.

| bench v2, 2,000 distractors | 0.1.8 (before) | 0.1.9 (after) |
|---|---|---|
| hit@5 | 0.674 | 0.674 |
| MRR@10 | 0.663 | 0.663 |
| precision@5 | 0.460 | 0.460 |
| weak queries returning nothing | 14/14 | 14/14 |
| holdout hit@5 / MRR | 0.200 / 0.125 | 0.200 / 0.125 |
| bench v1 hit@5 | 0.92 | 0.92 |
| bytes per search (mean) | 523 | 523 |

`scripts/bench.py --dump-scores` now also records the five highest candidate cosines per query.

## Breaking changes

`POST /cairn-memory/projects/init` with an unknown `project_id` now answers 409 unless the body has
`confirm_register: true`. `cairn init --yes` sends it. Nothing else changes on the wire.

## Upgrade steps

1. Upgrade the server and the clients together. There is no migration and no data wipe.
2. On each machine run `cairn update`.
3. Run `cairn doctor` and check that every item passes.
