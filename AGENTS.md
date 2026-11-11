# Cairn Memory Agent Instructions

These instructions apply to work in this project. Read `MEMORY.md` before
substantive work; OpenCode loads it through `opencode.jsonc`.

Before changing Rust, also read `RUST_IDIOMS.md`. It is the repository's
self-contained Rust quality rule and is loaded automatically by OpenCode.

## Working rules

These are the useful project-level parts of the Karpathy-inspired guidance:

- Inspect relevant code, documentation, callers, and checks before editing.
- State assumptions and ask when ambiguity would change the implementation.
- Prefer the smallest complete solution; avoid speculative abstractions and
  unrelated cleanup.
- Make surgical changes that preserve surrounding behavior and existing style.
- Define a verifiable success condition, run the narrowest relevant check, and
  report exactly what was checked.
- Keep Rust changes borrowing-oriented, explicit about errors and state
  transitions, and clean under rustfmt and Clippy unless a deliberate,
  reasoned exception is required.

## Cairn project conventions

- This is a Rust workspace with crates in `crates/core` and `crates/server`.
- Read `README.md` and `docs/README.md` for project orientation before making
  changes that affect product behavior or architecture.
- Documents under `docs/` follow `docs/standards.md`: one source of truth per
  topic, stable IDs, explicit lifecycle status, and evidence-backed claims.
- A change to a public contract, storage format, dependency direction, trust
  boundary, retrieval model, or synchronization behavior needs a corresponding
  ADR or ADR update.
- Use the project’s existing checks as appropriate:
  `cargo fmt --check`, `cargo test --workspace`,
  `./scripts/smoke_full_flow.sh`, `python3 scripts/measure_layer2.py`, and
  `./scripts/smoke_layer3.sh`. Do not run expensive or unrelated checks when a
  narrower check proves the requested result.
- Keep the product boundary clear: personal memory and branch continuity come
  before project promotion, collaboration, or federation.

## Critical verified lessons

- Sync event IDs are person-scoped. A duplicate must first resolve to a persisted
  sync event owned by the authenticated person; never acknowledge a collision
  from another person’s event. See `crates/core/src/lib.rs`,
  `crates/server/src/lib.rs`, and `scripts/smoke_layer3.sh`.
- REST smoke readiness must allow a clean Cargo build to finish before polling
  gives up. The current startup window is 60 seconds; do not reduce it to a
  short fixed timeout without accounting for first-build time. See
  `scripts/smoke_rest.sh`.

## Curated project memory

`MEMORY.md` is durable agent memory, not a transcript or task log.

Save a memory only when it is a verified, reusable fact that would prevent a
future agent from repeating a mistake. Good entries include:

- a settled architectural or product decision and its reason;
- a verified build, test, or runtime constraint;
- a recurring user correction or project preference;
- a confirmed failure mode or implementation gotcha; or
- a durable workflow rule that is not already obvious from the repository.

When saving memory:

- update an existing entry instead of creating a duplicate;
- include the date, type, status, reason, and evidence path or command;
- mark replaced knowledge as `superseded` or remove it when appropriate;
- never store secrets, tokens, private payloads, full transcripts, or temporary
  task progress; and
- do not duplicate large sections of `README.md`, `docs/`, or source code.

Current code and project documentation are authoritative over `MEMORY.md`.
When they conflict, verify the current source and correct the memory entry.
If no durable fact was learned, do not edit `MEMORY.md`.

## Source

The behavior principles above are adapted from the MIT-licensed
[Karpathy-inspired guidelines](https://github.com/multica-ai/andrej-karpathy-skills),
especially its `CLAUDE.md` and `skills/karpathy-guidelines/SKILL.md`.
