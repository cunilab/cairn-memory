---
id: CM-REL-003
title: "Cairn Memory 0.1.7 release notes"
status: draft
---

# Cairn Memory 0.1.7 release notes

Fixes from live agent tests of 0.1.6. No schema change, no new tools. Ranking amendment: [ADR 0015](./adr/0015-0.1.6-memory-model-and-agent-surface.md).

## What changed for users

- **Better ranking.** A search score is now absolute: word coverage and embedding similarity, not the position
  in a fused list, so a good match no longer loses to a poor one because of its kind or age. A just-saved note
  without an embedding yet ranks by its words alone. A hit that matches only on words and has an embedding
  must also be somewhat similar in meaning (cosine 0.35) to be returned.
- **Fewer stale memories after a "helpful" vote.** One vote per person: a later `helpful` takes back the same
  person's earlier `wrong`, and a memory made stale by feedback becomes active again. A memory that is stale
  because its files are gone is never revived by a vote.
- **Fewer duplicates.** Two saves with the same kind, title, summary and fields (case and spacing ignored)
  are one memory; the reason is no longer part of the identity. A correction that only changes the reason
  (`supersedes` pointing at the same memory) updates the reason instead of failing.
- **Cleaner handoffs.** Pasted commands, headings, quotes and fenced code are left out of the session handoff.
- **Rotated tokens.** `cairn mcp` re-reads its config and retries once after a 401, so a rotated device token
  works without restarting the agent.
- **Topic notes after a Read**, not only after an edit (same once-per-path and per-session caps).
- **Web console:** one setup command (agent choice, server address, copy) in Devices, the first-run checklist
  and the token dialog.
- **Project after a server wipe.** `cairn init` registers a committed project the server lost, with the same id;
  `cairn doctor` fails when the server does not list it.
- **Old Python wrapper cleanup.** `cairn setup` removes leftover 0.1.5 `run-hook` entries from the Claude and
  Codex hook files and deletes `hook/run-hook` and `hook/README.txt`, only when `run-hook` is Cairn's wrapper
  (it calls `cairn_hook.py`); a file of your own in the same place is kept.
- CI builds the server and web Docker images on every pull request; `CAIRN_SCHEMA` is documented in the deploy
  guide; `apps/web` has the release version, checked in CI.

## Benchmark (bench v2 main set, 2,000 distractors)

| | 0.1.6 | 0.1.7 |
|---|---|---|
| hit@5 | 0.651 | **0.674** |
| MRR@10 | 0.608 | 0.663 |
| precision@5 | 0.429 | 0.460 |
| direct hit@5 | 0.947 | 1.000 |
| weak queries empty | 14/14 | 14/14 |
| holdout weak empty / hit@5 | 4/5 / 0.20 | 5/5 / 0.20 |

Fixture set (v1): hit@5 0.92, MRR 0.92. Files: `bench/before-0.1.7-v2.json`, `bench/after-0.1.7-v2.json`,
`bench/after-0.1.7-v1.json`. The CI gate is hit@5 0.60 (v2) and 0.88 (v1).

## Breaking changes

None. Known limits: memories saved by 0.1.6 keep their old identity hash, so saving the same text again once
creates a duplicate that the automatic consolidation later merges. Paraphrases with no shared word still score low.

## Upgrade steps

1. Upgrade the server and the clients together (same version on both sides). There is no migration and **no data
   wipe**; start the 0.1.7 server on the existing database.
2. On each machine run `cairn update`.
3. `cairn setup` does not need to be run again unless `cairn doctor` warns about it.
4. Run `cairn doctor` and check that every item passes.
