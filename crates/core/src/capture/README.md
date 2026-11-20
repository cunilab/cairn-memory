# capture

## What
What arrives from a hook or a CLI call before it becomes a memory: observations, sessions, the git
work context, and redaction of secrets. Everything is redacted here before it is hashed or stored.

## Entry points
- `ObservationInput`, `Observation`, `CaptureResult`, `normalize_observation`.
- `WorkContext` (repo, branch, worktree, commit), `Session` and `Session::is_idle`.
- `redact_sensitive_content`, `redact_work_context`, `contains_private_key_header`.

## Depends on
Nothing in `cairn-core` except `error` and `id`.
