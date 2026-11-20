---
id: CM-RESEARCH-001
title: "Survey — Memory Confidence, Relation, and Correction"
status: draft
---

# Survey — Memory Confidence, Relation, and Correction

| Field | Value |
| --- | --- |
| Status | Draft survey. This is a record of source reading. It is not an accepted design, and it does not change [CM-CONTRACT-004](../archive/contracts/lifecycle-v1.md). |
| Read on | 2026-11-11 |
| Question | How do other memory systems score confidence, decide that a memory is related or unrelated, and keep or replace an old record when it is corrected? |
| Cairn today | Confidence is stored as `0..100`. Observations default to `50`. Explicit memories default to `80`. `correct` writes a new memory with `supersedes`, keeps the old record, and copies `project_id` when the target has one. Normal recall hides the old record while the correction is live. The exportable record remains. Forget, not correction, writes the tombstone. |

No source below documents `75` as the line between a related memory and an unrelated memory. A missing cutoff in a source is left missing. It is not treated as `0` or as `75`.

## How this was read

Two passes produced this note.

1. Direct reads of AgentMemory `v0.9.30` `src/functions/relations.ts` and `src/types.ts`, AgentMemory `main` `src/functions/retention.ts` and `src/types.ts`, and Graphify's published confidence pages. `v0.9.30` is the snapshot named in [CM-CONTRACT-001](../contracts/agentmemory-baseline.md).
2. A later cross-checked web survey of AgentMemory, Mem0, Zep/Graphiti, Letta, and the nearby systems named in the coverage section. That survey's own status is partial. Its gaps are copied at the end so they are not forgotten.

Where the two passes describe the same AgentMemory function, the formula matches. Small `v0.9.30` versus `main` differences are called out in the AgentMemory section.

## What Cairn already stores

[CM-CONTRACT-004](../archive/contracts/lifecycle-v1.md) keeps these dimensions separate:

| Dimension | Range in the contract | Default recorded there |
| --- | --- | --- |
| Importance | `0..100` | Hint words `low/medium/high/critical` map to `20/50/80/100`. Explicit memories use `80` when no value is supplied. |
| Confidence | `0..100` | `50` for observations. `80` for explicit memories. Session memories average the source observations. |
| Freshness | `0..100` | New items default to `100`. |

[CM-SPEC-002](../specs/memory-context-and-promotion.md) defines confidence as how strongly the evidence supports the memory. It is not importance, freshness, authority, or a search score. Authorization is never a score.

[ADR 0010](../adr/0010-correction-keeps-linked-history.md) is the current correction rule. `correct` inserts a durable `correction` memory whose `supersedes` field points at the old memory or observation. It does not tombstone the target and does not drop `project_id`. The old record stays in storage and export. Normal recall skips it while a live correction supersedes it. When the target memory has a `project_id`, the correction copies that `project_id`.

The file-backed and PostgreSQL `correct` paths follow that decision: the replacement is inserted, the target stays stored, and the replacement keeps the target's `project_id`. A normal correction therefore leaves the superseded project memory stored while project listing shows the live correction. `forget` adds a tombstone and leaves indexed project entities `active`. The SQL status `retired` exists in `migrations/0007_layer4_registry_maintenance.sql` and is not written by the Rust paths read for this survey.

[CM-SPEC-003](../specs/graph-assisted-retrieval.md) already describes a memory-graph edge, "decision supersedes prior decision," with provenance, confidence, scope, actor, and created-at on the edge. That specification is still draft. The implemented correction path keeps the old record, copies `project_id`, and stores `supersedes` on the new correction. It does not tombstone the target.

## AgentMemory

Repository: [rohitg00/agentmemory](https://github.com/rohitg00/agentmemory). The relation and type reads used the tag `v0.9.30`. The retention read used `main` on 2026-11-11. The cross-check also used `main` for `src/functions/remember.ts`, `src/functions/lessons.ts`, and `src/functions/relations.ts`.

### A durable memory has no confidence field

`Memory` in `src/types.ts` carries `strength`, `version`, `isLatest`, `parentId`, `supersedes`, and `relatedIds`. It does not carry `confidence`.

Confidence exists on other records:

| Record | Field | Scale found in the source |
| --- | --- | --- |
| `MemoryRelation` | `confidence?` | `0..1` when computed or supplied |
| `CompressedObservation` | `confidence?` | Present as a number. The files read do not document a `0..100` scale or the defaults `50` and `80`. |
| `SemanticMemory` | `confidence` | Number. Retention treats it as a `0..1` salience input. |
| `Lesson` and `Insight` | `confidence` | Number, with separate decay fields |
| `EdgeContext` on a knowledge-graph edge | `confidence?` | Optional number |
| `GraphEdge` | `weight` | The extraction prompt asks for `0.1-1.0` |

Relation types are `supersedes`, `extends`, `derives`, `contradicts`, and `related`.

### How relation confidence is computed

`computeConfidence` in `src/functions/relations.ts` is the same on `v0.9.30` and on `main`:

1. Start at `0.5`.
2. Add `0.1` for each session id shared by the two memories, capped at `+0.3`.
3. Add `0.1` when both memories were updated within 7 days.
4. Subtract `0.1` when both were updated more than 90 days ago.
5. Add `0.1` when the relation type is `supersedes`.
6. Subtract `0.05` when the relation type is `contradicts`.
7. Clamp the result to `[0, 1]`.

`mem::relate` uses that function when the caller omits confidence. An explicit confidence is clamped to `[0, 1]` and stored as given. The function writes one `MemoryRelation` and, when the id is absent, appends each memory's id to the other's `relatedIds`. Both memory rows stay stored.

`mem::get-related` walks `relatedIds`, `supersedes`, `parentId`, and relation rows. The default hop limit is 2, and the hard cap is 5. When `minConfidence` is omitted, the floor is `0`, so a low-confidence link is still returned. A caller can raise the floor. The walk keeps the highest confidence among the matching relation rows, or `0.5` when no row matches. Results are sorted by confidence, highest first. A link below the caller's floor is omitted from that response. The stored memory is not deleted.

The lesson skill, read from `main` `src/functions/lessons.ts` during the cross-check, tells the agent to send `0.7` for a direct user correction and `0.5` for a self-observed pattern. A new lesson stores `0.5` when the supplied value is missing or outside `[0, 1]`. `mem::lesson-recall` uses `0.1` when its minimum is omitted. Those are optional retrieval filters. They are not a classification of a memory as unrelated.

### Save-time duplicate matching

`src/functions/remember.ts` on `main` uses token Jaccard similarity against the latest memory for the same agent:

| Similarity | Effect recorded in the source |
| --- | --- |
| Strictly greater than `0.7` | Supersede that latest memory. |
| Strictly greater than `0.4` and not greater than `0.7` | Return a `similarTo` hint. Do not supersede. |
| Otherwise | Save independently. |

The changelog phrase "0.4 to 0.7" is looser than the code. The code requires similarity strictly greater than `0.4` and strictly greater than `0.7`.

That automatic supersession keeps the old memory, marks it non-latest, and removes it from the BM25 and vector indexes. The new record is the next version, with `parentId` and `supersedes` set to the old id. The old row remains in the version chain. This automatic path links versions through those fields. The cross-check found that the `supersedes` relation row at confidence `1.0` is written by `mem::evolve`, not by this automatic save path.

### Explicit evolve

`mem::evolve` on both `v0.9.30` and `main` does the following:

1. Reject a missing memory, an empty content change, an unknown type, a no-op edit, or a record that is already `isLatest === false`.
2. Create a new id. Set `version` to the old version plus one, `parentId` to the old id, `isLatest` to true, and `supersedes` to the old id followed by whatever that record already superseded.
3. Set the old record's `isLatest` to false and leave the row in the key-value store.
4. Remove the old id from the BM25 index and the vector index.
5. Index the new memory.
6. Write a `MemoryRelation` of type `supersedes` from the new id to the old id with confidence `1.0`.
7. Trigger `mem::cascade-update` for the superseded id.

On `main`, the old row's `updatedAt` is passed through `supersededAt` before it is saved. The `v0.9.30` function read here does not do that. On `main`, the vector text is built by `observationEmbedText`. The `v0.9.30` function concatenates title and content. The confidence value `1.0` and the non-latest retention are the same in both reads.

Normal recall excludes superseded versions. The project README states that the version chain keeps their history. Graph retrieval skips a memory or edge whose `isLatest` is false.

### Retention is a different score, and it can delete

`src/functions/retention.ts` on `main` computes a separate `0..1` retention score:

```text
score = min(1, salience * temporalDecay + reinforcementBoost)
temporalDecay = exp(-lambda * ageInDays)
```

The default configuration is `lambda = 0.01`, `sigma = 0.3`, and these tiers:

| Tier | Default range |
| --- | --- |
| Hot | `score >= 0.7` |
| Warm | `0.4 <= score < 0.7` |
| Cold | `0.15 <= score < 0.4` |
| Evictable | `score < 0.15` |

Salience for an episodic memory starts from its type: architecture `0.9`, preference `0.85`, pattern `0.8`, bug `0.7`, workflow `0.6`, fact `0.5`, otherwise `0.5`. A semantic memory can raise that base to its own `confidence`. Access count adds at most `0.2`. Only `isLatest` episodic memories are scored. `mem::retention-evict` deletes records below the threshold, which defaults to the cold line `0.15`, up to a bounded batch. That is a real delete of the memory row, its retention score, and its access log.

`mem::forget` deletes the key-value record. `POST /agentmemory/forget` is documented as deleting observations. `memory_governance_delete` deletes selected memories and requires an explicit confirmation in the forget skill. Auto-forget can expire a memory past `forgetAfter`. When token overlap between two latest memories exceeds `0.9`, auto-forget marks the older one non-latest and drops it from the indexes. That overlap path does not set `parentId` or `supersedes`.

### Knowledge-graph edges

The extraction prompt in `src/prompts/graph-extraction.ts` asks for entity types and relationship types `uses`, `imports`, `modifies`, `causes`, `fixes`, `depends_on`, and `related_to`, with a weight from `0.1` to `1.0` for how strong or direct the connection is. Stored `GraphEdge` values also include temporal fields `tvalid` and `tvalidEnd`, `supersededBy`, `isLatest`, and `stale`. Traversal skips a stale edge and an edge that is not latest, and it orders neighbors by weight.

Search itself is BM25, plus a vector score when embeddings are enabled, plus a graph score when graph data exists. Those are fused. "Unrelated" in the recall documentation means the match did not fit inside the candidate budget. It is not a confidence cutoff.

## Mem0

Sources: [How Mem0 works](https://docs.mem0.ai/core-concepts/how-it-works), [search API](https://docs.mem0.ai/api-reference/memory/search-memories), [v2 to v3 migration](https://docs.mem0.ai/migration/platform-v2-to-v3), and `mem0/memory/main.py` plus `mem0/configs/prompts.py` on `main`.

Mem0's add and search code defines no stored numeric memory-confidence scale. A search hit has a score on `[0, 1]`. That score is retrieval similarity. `Memory.search` defaults its threshold to `0.1`. The v2 comparison named in the migration note is `0.3`. Entity identity requires a score of at least `0.95` or an exact normalized-text match. Extraction links memories by qualitative criteria and does not link memories that only share a vague theme.

An automatically extracted change adds the new fact and leaves the prior fact stored. It does not delete, tombstone, or overwrite the old memory. The extraction prompt asks the model to put the existing memory id in `linked_memory_ids` when the memories are the same entity or topic, an updated preference, a continuation, or a contradiction, and to omit those ids when the memories are unrelated. The open-source add path that was inspected does not copy `linked_memory_ids` into the stored payload. Whether the hosted product persists them was not visible in the public files.

An explicit update keeps the same memory id, overwrites its vector payload, and records the previous text in history as `UPDATE`. An explicit delete removes the vector and writes history with `DELETE` and `is_deleted = 1`. Memories that mention the same extracted entity are connected through that entity's `linked_memory_ids`.

The platform update schema includes `replaced_by` and `lifecycle_state`. The inspected update docs and the open-source update path do not show when `replaced_by` is written. `DEFAULT_UPDATE_MEMORY_PROMPT` still describes in-place `UPDATE` and `DELETE`, and `Memory.add`'s docstring still mentions those events, but the current `Memory.add` does not call that prompt.

A search snippet for a Mem0 cookbook mentioned `0.4`, `0.6`, `0.8`, and a `0.7` confidence gate. The fetched page did not contain those sections, so those numbers are not used here.

## Zep and Graphiti

Sources: [Graphiti README](https://github.com/getzep/graphiti/blob/main/README.md), `graphiti_core/utils/maintenance/edge_operations.py`, `graphiti_core/llm_client/decision_prompts.py`, [Zep facts](https://help.getzep.com/facts), and [Zep fact ratings](https://help.getzep.com/v2/facts.md).

Graphiti stores facts as edges with episodes, `valid_at`, `invalid_at`, and `expired_at`. A contradicted edge is not deleted. It stays in the graph. `invalid_at` is set to the new edge's `valid_at`, and `expired_at` is set to the current time when it was unset. The resolver can fill an in-memory `invalidated_by` map from each invalidated edge UUID to the UUID of the edge that invalidated it. The inspected `EntityEdge` model does not show a persisted `INVALIDATED_BY` graph edge. Current `main` also sets `expired_at` to now when a resolved edge already has `invalid_at`. Issue 2012 reports that this expires future-ended facts at write time. Whether that remains unfixed was not re-tested.

Jev, the optional edge resolver, scores duplicate and contradiction as probabilities. It keeps a fact only when the duplicate probability is at least `0.7` or the contradiction probability is at least `0.8`. No default related-versus-unrelated cutoff is documented.

Zep fact ratings are a caller-defined relevance scale from `0` to `1`. A shown `min_rating` of `0.7` is an example retrieval filter after a custom high, medium, and low instruction. It is not a default unrelated threshold. Zep separates world time from the time a change was learned through `created_at`, `valid_at`, `invalid_at`, and `expired_at`. A new statement can invalidate the prior fact and add new facts.

## Letta

Sources: Letta `0.16.8` `letta/schemas/passage.py`, `letta/functions/function_sets/base.py`, and `letta/orm/sqlalchemy_base.py`, plus the [archival memory guide](https://docs.letta.com/v1-sdk/memory/archival-memory/) and [MemFS](https://docs.letta.com/concepts/memfs).

The archival passage model has no numeric confidence score and no relatedness threshold. `is_deleted` defaults to false. Archival search ranks by semantic similarity and defaults `top_k` to 10. That is a result count, not a cutoff.

A core-memory correction overwrites the same block. The matched span is replaced in place, or deleted when the new string is empty. The old text is not stored as a separate linked record. Archival deletion is a flag on the same row: soft delete sets `is_deleted` to true, and hard delete removes the row. An update changes text on the existing passage id. Agents cannot revise archival memory themselves. An insert persists until a developer updates or deletes it through the SDK. The public archival `DELETE` route was not traced, so this note does not claim which delete function that route calls. `PassageUpdate`'s service body was not opened, so a hidden insert-and-link path is not ruled out by the handler schema alone.

MemFS records current memory updates as git commits of the edited files. Prior content stays in repository history. That history is not a tombstone, a non-latest flag, or a `supersedes` link between memory records.

The MemGPT paper's context-window warning and Letta's memory-pressure setting are context controls. They are not a relatedness threshold.

## Graphify

This section is from the direct read on 2026-11-11. The later cross-check did not re-verify it.

[Graphify's how-it-works note](https://github.com/Graphify-Labs/graphify/blob/v8/docs/how-it-works.md) tags every relationship `EXTRACTED`, `INFERRED`, or `AMBIGUOUS`. `EXTRACTED` edges have confidence `1.0`. `INFERRED` edges use this rubric:

| Score | Meaning published there |
| --- | --- |
| `0.95` | Near-certain. An explicit cross-file reference with one plausible target. |
| `0.85` | Strong evidence. Naming and context align. |
| `0.75` | Reasonable. Contextual, but not explicit. |
| `0.65` | Weak. Naming similarity only. |
| `0.55` | Speculative. |

`0.75` in that rubric means a reasonable inferred edge. It is not a rule that drops a node or a memory. When the same relation between the same nodes is found again, the edge keeps the highest-confidence copy and lists the other copies from weakest to strongest.

`graphify-core` `confidence.rs`, as published at the time of the read, maps the labels to default scores `1.0` for extracted, `0.7` for inferred, and `0.4` for ambiguous. The [provenance doc](https://docs.graphify.com/concepts/provenance) says an inferred edge can exist without a model, through static name and import resolution, and that the reader should check the evidence and any confidence score before treating an inferred edge as a fact.

Cairn's graph specification already points at Graphify for extracted-versus-inferred edges. It does not copy this `0.0..1.0` rubric into the lifecycle confidence field.

## Systems checked only far enough to avoid a false cutoff

The cross-check looked at these and refused to promote them into a Cairn rule:

| System | What was actually seen | What was not established |
| --- | --- | --- |
| Cognee | `.env.template` sets `CONFIDENCE_THRESHOLD=0.8` under translation. Rewriting a `DataPoint` with the same id overwrites the node. Conflict review can keep both facts. `forget()` deletes permanently. | `0.8` is not a memory relatedness cutoff. The conflict-review behavior was not given a numeric threshold here. |
| Supermemory | An older v4 search page shows a threshold default of `0.6`. Current v5 search docs say `0.3`. An example metadata value `"confidence": 0.9` appears in a sample. | The sample is not a documented confidence scale. The two search defaults were not re-tested against one another beyond the two docs. |
| Hindsight | Older opinions used a `0.0..1.0` confidence. Current observations track evidence. | No default related-versus-unrelated numeric threshold was verified, so no Hindsight number is used. |

## What this means for Cairn

This section is a design note from the survey. It is not an implementation decision and it does not amend the lifecycle contract.

The behavior shared by AgentMemory's evolve path and Graphiti's edge invalidation is the closest match to keeping history visible:

- The old record stays stored.
- It is marked no longer current. AgentMemory uses `isLatest = false` and index removal. Graphiti uses `invalid_at` and `expired_at`.
- The new record points at the old one. AgentMemory stores `parentId`, a `supersedes` list, and a relation at confidence `1.0`. Graphiti can record the invalidating edge UUID in an in-memory map.
- Normal retrieval returns the current record. History remains available through the version chain or the invalidated edge.

Mem0's extracted-add path also leaves the prior fact stored, and its prompt asks for `linked_memory_ids`, but the inspected open-source writer drops those ids. Its explicit update overwrites one id. Letta overwrites or flags the same row. Those two do not give Cairn a separate old record plus a link.

A below-75 rule on Cairn's existing confidence field is not supported by these sources. It would also hide default observations, which the lifecycle contract stores at confidence `50`, while explicit memories stored at `80` would remain. AgentMemory's nearest numbers are a duplicate Jaccard above `0.7`, a lesson-correction hint of `0.7`, a retention hot tier of `0.7`, and a delete line of `0.15`. Graphify's `0.75` describes one inferred-edge grade. Jev's `0.7` and `0.8` are probabilities required before accepting a duplicate or a contradiction. Zep's `0.7` is an example filter. None of those is a stored memory-confidence cutoff of 75.

The lifecycle contract would have to change before Cairn stopped tombstoning a corrected memory. That change needs its own review. This survey only records the evidence for that review.

[CM-ADR-0010](../adr/0010-correction-keeps-linked-history.md) is the decision that followed this survey: a correction keeps the old memory stored and linked, normal recall returns the new memory, and there is no below-75 cutoff.

## Coverage gaps

These limits were recorded by the cross-check and are part of the survey:

- No AgentMemory source says that its keep-and-link behavior would improve Cairn. That judgment is local to the section above.
- Hosted Mem0 persistence of `linked_memory_ids` and the write path for `replaced_by` were not visible.
- Graphiti's `invalidated_by` map was not shown as a persisted edge.
- Letta's public delete route and `PassageUpdate` service body were not fully traced.
- Six candidate claims were dropped before verification because they were malformed or over the survey cap.
- Retention eviction was read from AgentMemory `main`, not re-read on the `v0.9.30` tag.
- Graphify was read directly and was not part of the later cross-check.

## Sources

- AgentMemory `v0.9.30` and `main`, `src/functions/relations.ts`: <https://raw.githubusercontent.com/rohitg00/agentmemory/v0.9.30/src/functions/relations.ts>
- AgentMemory `main`, `src/types.ts`: <https://raw.githubusercontent.com/rohitg00/agentmemory/main/src/types.ts>
- AgentMemory `main`, `src/functions/retention.ts`: <https://raw.githubusercontent.com/rohitg00/agentmemory/main/src/functions/retention.ts>
- AgentMemory `main`, `src/functions/remember.ts`: <https://raw.githubusercontent.com/rohitg00/agentmemory/main/src/functions/remember.ts>
- AgentMemory `main`, `src/functions/lessons.ts`: <https://raw.githubusercontent.com/rohitg00/agentmemory/main/src/functions/lessons.ts>
- AgentMemory `main`, `src/prompts/graph-extraction.ts`: <https://raw.githubusercontent.com/rohitg00/agentmemory/main/src/prompts/graph-extraction.ts>
- Mem0 docs and `mem0/memory/main.py`, `mem0/configs/prompts.py` on `main`
- Graphiti `graphiti_core/utils/maintenance/edge_operations.py` and `graphiti_core/llm_client/decision_prompts.py` on `main`
- Zep facts docs: <https://help.getzep.com/facts> and <https://help.getzep.com/v2/facts.md>
- Letta `0.16.8` passage schema, core-memory tools, and ORM base
- Graphify v8 `docs/how-it-works.md` and `graphify-core` `confidence.rs`
- Cairn [CM-CONTRACT-004](../archive/contracts/lifecycle-v1.md), [CM-SPEC-002](../specs/memory-context-and-promotion.md), and [CM-SPEC-003](../specs/graph-assisted-retrieval.md)
