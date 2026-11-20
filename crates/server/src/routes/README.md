# routes

## What
The REST handlers, one file per area. Each file owns its request and response types and calls one or two
`Store` methods; `mod.rs` only lists paths. Every path is under `/cairn-memory`.

## Entry points
- `health`: `/health`, `/livez`. `account`: `/auth/*`. `device`: `/devices*`. `admin`: `/admin/jobs*`.
- `project`: `/projects*`. `capture`: `/sync/push`, `/observe`, `/session/*`.
- `memory`: `/remember`, `/memories*`, `/feedback`, `/forget`, `/delete`, `/links*`, `/supersede`, `/contradict`.
- `retrieve`: `/search`, `/smart-search`, `/context`, `/recall`, `/resume`, `/unresolved`, `/why-changed`,
  `/graph`, `/export`, `/diagnose`.

## Depends on
`crate::auth` (extractors), `crate::error` (`{error, code}`), `cairn-store`, `cairn-core`.
