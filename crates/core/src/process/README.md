# process

## What
The deterministic work done after a save or a capture, with no model call: turning a `SaveInput` into
a stored memory, deriving entities, distilling a session into a handoff, building the embedding text,
choosing consolidation candidates, and job retry rules.

## Entry points
- `prepare_save` (redact, validate, kind policy, hashes, entities), `Origin`, `NewMemory`.
- `entities::derive`, `distill::distill` (typed notes, fail-then-fix gotcha, test-command fact, handoff) and `handoff_id` (per owner, project or repository, branch, worktree).
- `embed::embedding_text`, `source_hash`, `validate_vector`, `model_label`.
- `consolidate::{band, guard}` (branch and origin guards); `jobs::{JobKind (embed, distill, consolidate, link), JobEnd, backoff_after}`.
- `entities::derive` is the one extractor for save, the link job and the search entity channel: file, module, symbol, test, command, service, term (docs/architecture.md).
- Agent saves also pass `memory::validate::validate_quality` (`Error::Quality`, HTTP 400 `quality`).

## Depends on
`memory`, `capture`.
