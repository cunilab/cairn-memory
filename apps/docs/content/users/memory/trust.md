---
id: CM-DOC-USERS-MEMORY-TRUST
title: "Trust and corrections"
status: accepted
description: "Read Cairn’s trust labels, correct outdated knowledge and forget your own records."
---

# Trust and corrections

The [trust label](../../glossary.md#trust-level) reflects a record’s status, rank, and feedback.

```mermaid
flowchart LR
  accTitle: Memory trust from candidate to proven
  accDescr: Distillation can produce a new candidate. A verified save records normal trust. Helpful feedback or repeated saves can raise it to proven.
  New[New] --> Normal[Normal] --> Proven[Proven]
```

Wrong votes flag a record as doubted. Superseding and forgetting change its status separately.

## Trust labels

Cairn evaluates confidence in order:

1. **[Doubted](../../glossary.md#doubted)**: Displayed after a wrong vote or when the record is stale. Two wrong votes from different users, or one from the owner, mark a record [stale](../../glossary.md#stale) and lower its rank.
2. **[New](../../glossary.md#new)**: Distilled candidate awaiting verification. Ranks lower and stays out of L1.
3. **[Proven](../../glossary.md#proven)**: Validated with at least two helpful votes or three separate saves of the same knowledge. Prioritized in L1 briefs.
4. **[Normal](../../glossary.md#normal)**: Standard verified memory.

## Corrections, links and forgetting

- **Supersede**: Pass `supersedes: "<old_id>"` when saving an update. Links historical records and prevents stale advice.
- **Contradict**: Save a `contradicts: "<id>"` link when disputing a record owned by another person or project.
- **Forget**: Call `memory_forget` or `cairn forget <id>` to hide an owned memory from normal recall. Age alone never deletes or hides records.

## Read the counters

Inspect memory confidence metrics in the web console sheet:

- **Shown**: Total times injected into session briefs or contextual tool replies.
- **Used**: Explicit agent fetches, searches, and unprompted gets.
- **Detail**: Requests for full long-form technical content.

See [Recall layers](layers.md) for automatic recall eligibility and muting rules.
