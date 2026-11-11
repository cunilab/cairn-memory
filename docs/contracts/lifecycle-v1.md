---
id: CM-CONTRACT-004
title: "Layer 1 Lifecycle Classification"
status: draft
---

# Layer 1 Lifecycle Classification

Cairn stores lifecycle dimensions independently. They describe relevance and
risk; they are not authorization and cannot broaden visibility.

## Dimensions

| Dimension | Values | Layer 1 behavior |
| --- | --- | --- |
| Importance | `0..100` | Top-level `importance` hints map `low/medium/high/critical` to `20/50/80/100` |
| Sensitivity | `public`, `private`, `sensitive`, `secret` | Secret-like patterns classify as `secret`; private/confidential language classifies as `sensitive` |
| Confidence | `0..100` | Explicit input is preserved; default is `50` for observations and `80` for explicit memories |
| Freshness | `0..100` | Explicit input is preserved; new items default to `100` |
| Retention | `temporary`, `durable` | Raw observations default to temporary; session summaries and explicit memories default to durable |
| Expiration | Unix seconds in `expiresAt` | Expired memories are excluded from recall and counted by diagnostics |

## Input shape

Lifecycle-aware capture and explicit save requests may include:

```json
{
  "lifecycle": {
    "importance": 80,
    "sensitivity": "sensitive",
    "confidence": 90,
    "freshness": 70,
    "retention": "temporary",
    "expiresAt": "1800000000"
  }
}
```

The existing top-level observation `importance` string remains supported for
AgentMemory compatibility. A nested numeric lifecycle importance takes
precedence when both are present.

## Recall and diagnostics

- Recall results include their lifecycle classification.
- Memories also carry a `kind`: `session_summary`, `explicit`, `lesson`, or
  `correction`.
- Session-derived memories aggregate the highest importance and sensitivity,
  average confidence, and durable retention from their source observations.
- `GET /cairn-memory/diagnose` reports temporary observations, durable memories,
  sensitive items, and expired items.
- MCP `memory_diagnose` returns the same lifecycle summary.
- A default expiration duration is intentionally not chosen yet; the policy
  decision remains open. The local Layer 1 policy uses a 30-day default for
  temporary raw observations, while explicit `expiresAt` values are honored.

Corrections create a new durable `correction` memory with `supersedes` pointing
to the old memory or observation. The old item stays stored and linked. Normal
recall skips it while another memory for the same person has `supersedes` set
to its id and that newer memory is not forgotten and not expired. Forget still
tombstones. The correction's confidence is `100`. A source survey of how other
systems score confidence and keep or replace a corrected record is
[CM-RESEARCH-001](../research/memory-confidence-and-correction.md). The accepted
decision is [CM-ADR-0010](../adr/0010-correction-keeps-linked-history.md).

Older version-1 store records without lifecycle fields load with safe defaults,
so adding this classification does not invalidate existing local stores.
