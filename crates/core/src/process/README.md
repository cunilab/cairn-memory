# process

## What
The deterministic work done after a save or a capture, with no model call: turning a `SaveInput` into
a stored memory, deriving entities, distilling a session into a handoff, building the embedding text,
choosing consolidation candidates, and job retry rules.

## Entry points
- `prepare_save` (redact, validate, kind policy, hashes, entities), `Origin`, `NewMemory`.
- `entities::derive`, `distill::distill` and `handoff_id`.
- `embed::embedding_text`, `source_hash`, `validate_vector`, `model_label`.
- `consolidate::{band, cosine, same_cohort, newer}`; `jobs::{JobKind, JobEnd, backoff_after}`.

## Depends on
`memory`, `capture`.
