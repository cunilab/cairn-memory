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
| Sensitivity | `public`, `private`, `sensitive`, `secret` | Secret patterns below classify as `secret`; private or confidential language classifies as `sensitive`; otherwise `private`. The stored label is at least the detected label |
| Confidence | `0..100` | Explicit input is preserved; default is `50` for observations and `80` for explicit memories |
| Freshness | `0..100` | Explicit input is preserved; new items default to `100` |
| Retention | `temporary`, `durable` | Raw observations default to temporary; session summaries and explicit memories default to durable |
| Expiration | Unix seconds in `expiresAt`, a decimal string from `0` through `9223372036854775807` (`i64::MAX`) | Larger or non-numeric values are rejected as validation errors on capture, remember, sync push, and import. Expired memories are excluded from recall and counted by diagnostics |

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

## Sensitivity floor

Classification reads the original content. Capture, explicit remember, and
correction also read the original concepts and the original human-readable
`WorkContext` lists: `files`, `symbols`, `commands`, `tests`, `services`,
`handoffs`, and `runbooks`. Redaction of that text runs before persistence.
The detected label is the floor. A caller may supply an equally strict or
stricter label. A weaker label is raised, including `public` on text that
detects as `private`. Repository, branch, worktree, commit, and identity or
provenance fields are not redacted and are not part of this floor. A supported
secret in one of those fields is unchanged.

These patterns detect `secret`:

- `authorization: bearer `, `api_key=`, `token=`, and `password=`, matched
  without case sensitivity. The marker stays and the following token becomes
  `[REDACTED]`.
- A case-insensitive `sk-` token through the next whitespace, replaced by
  `[REDACTED]`. The `sk-` must start a token: at the start of the text or
  after a character that is not a letter or digit, so words such as
  `disk-full` and `task-queue` are kept.
- A PEM header `-----BEGIN <label>-----` whose label is `PRIVATE KEY`,
  `RSA PRIVATE KEY`, `EC PRIVATE KEY`, `OPENSSH PRIVATE KEY`, or
  `ENCRYPTED PRIVATE KEY`, through the matching `END` marker. A header with no
  matching `END` removes that header line and following base64 or OpenSSL
  `Proc-Type` / `DEK-Info` lines. A line of optional PEM base64 glued to
  `-----END <label>` without the closing `-----` is a footer: those glued
  bytes and that footer token are removed, and text after the token stays.
  A later line that is not a PEM continuation stays. A `BEGIN` that never
  closes with `-----`, and labels outside these five, are not this pattern.

`BEGIN CERTIFICATE`, `BEGIN PUBLIC KEY`, and the words `PRIVATE KEY` without
that header are not secret patterns and are not redacted. The words `private`,
`confidential`, `internal-only`, and `personal data` still detect as
`sensitive`. Capture, explicit remember, correction, and sync normalize use
this floor for content, concepts, and those `WorkContext` lists.

Import redacts observation and memory content, those `WorkContext` lists, and
memory `next_steps`, `unresolved_work`, and `promotion_reason`. It keeps the
bundle's sensitivity label and does not apply this floor. A kept `public`
label is the bundle's label after those strings were redacted. It does not
mean the original metadata was free of secrets. Already stored rows are not
rewritten. Re-import compares the redacted bundle with the stored row, so a
stored row that still has the raw string conflicts instead of being rewritten.

A session row has no sensitivity field. Session start redacts the same
`WorkContext` lists before persistence and leaves repository, branch,
worktree, and commit as sent. A later empty-context capture inherits those
already redacted strings. Its floor sees the stored text, not the original
secret, so a secret that existed only on the started session does not raise
the observation label. The secret bytes are not stored. A secret in the
capture, remember, or correction request itself still raises that record's
label. A new session summary copies the stricter source label and redacts the
human-readable lists and handoff text it writes. It does not rescan a secret
that was already removed from an older observation. Distilled notes inherit
that lifecycle and redact the same metadata fields. Source rows stay as stored.

## Recall and diagnostics

- Recall results include their lifecycle classification.
- Memories also carry a `kind`: `session_summary`, `handoff`, `explicit`, `lesson`,
  `correction`, `decision`, `rule`, `gotcha`, `procedure`, `fact`, `preference`, or `capture`. Structured fields and scopes are defined in [the scope contract](../../contracts/project-layer4.md#memory-scopes-and-record).
- Session-derived memories aggregate the highest importance and sensitivity,
  average confidence, and durable retention from their source observations.
- `GET /cairn-memory/diagnose` reports temporary observations, durable memories,
  sensitive items, and expired items. Once the file store has any account it
  needs an account bearer and counts only that account's rows, as PostgreSQL
  does; a store with no accounts counts every row.
- MCP `memory_diagnose` returns the same lifecycle summary.
- A default expiration duration is intentionally not chosen yet; the policy
  decision remains open. The local Layer 1 policy uses a 30-day default for
  temporary raw observations, while explicit `expiresAt` values are honored.

Corrections retain the old record and create a live successor with `supersedes`, keeping the target's scope, typed fields and author and recording the actor. Only authorized same-scope successors suppress a target. Project author/owner and global author/admin checks apply. A successor's deletion may reveal its predecessor. The correction's confidence is `100`. See [ADR 0010](../../adr/0010-correction-keeps-linked-history.md).

## Forgetting shared knowledge

Authenticated forgetting or retained deletion of a project note hides only that
record. This applies to session notes, handoffs, typed notes, explicit project
notes and captures exposed as memory records: shared source observations and
sibling knowledge remain. Global-note forgetting also hides only that record.
Author/owner (project) and author/admin (global) authorization still applies.
Both stores implement this boundary.

Explicitly forgetting a source event hides that capture and every record derived
from it. A session end may refresh its summary and handoff from remaining retained
captures, but never resurrects a forgotten or superseded note. Private personal
memory forgetting retains its owned-source cascade. A source ID in metadata alone
never grants authority over another person's capture.

The shared project model is pre-release and requires fresh development stores;
no compatibility or migration of older project data is provided.
