# routes

## What
The REST handlers, one file per area. Each file owns its request and response types and calls one or two
`Store` methods; `mod.rs` only lists paths. Every path is under `/cairn-memory`.

## Entry points
- `health`: `/health`, `/livez`. `account`: `/auth/*`. `device`: `/devices*`. `admin`: `/admin/jobs*`, `/admin/fixes*`, `/admin/searches`, memory rewrite.
- `fixes`, `instructions`, `stats`: device fixes, agent instructions and console statistics.
- `project`: `/projects*`. `capture`: `/observe`, `/session/*`.
- `memory`: `/remember`, `/get`, `/memories*` (including pins), `/feedback`, `/forget`.
- `retrieve`: `/search`, `/session/brief`,
  `/session/topic`, `/session/command-recall`, `/session/prompt-recall`, `/graph`.
- `skill`: `/skills*` (search, get, save, feedback, history, rollback, archive and suggestions).

## Depends on
`crate::auth` (extractors), `crate::error` (`{error, code}`), `cairn-store`, `cairn-core`.
