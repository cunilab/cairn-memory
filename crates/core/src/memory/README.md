# memory

## What
The memory model and the rules that make a save valid: kinds, scopes, statuses, links, the typed
`Fields` per kind, identity and evidence hashes, and the default-scope policy. It is the
base of `cairn-core`: pure functions and types, no IO.

## Entry points
- Types: `Memory`, `SaveInput`, `Context`, `Kind`, `Scope`, `Status`, `Relation`, `Link`, `Entity`, `Usage`, `Fields`.
- `validate_save`, `validate_fields`, `Fields::parse`.
- `identity_hash`, `evidence_hash`, `normalize_evidence`.
- `default_scope`, `resolve_scope`, `expires_at` (currently always `None`).

## Depends on
Nothing in `cairn-core` except `error` and `id`.
