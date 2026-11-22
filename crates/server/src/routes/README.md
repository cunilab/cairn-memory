# routes

## What
The REST handlers, one file per area. Each file owns its request and response types and calls one or two
`Store` methods; `mod.rs` only lists paths. Every path is under `/cairn-memory`.

## Entry points
- `health`: `/health`, `/livez`. `account`: `/auth/*`. `device`: `/devices*`. `admin`: `/admin/jobs*`.
- `project`: `/projects*`. `capture`: `/observe`, `/session/*`.
- `memory`: `/remember`, `/get`, `/memories*`, `/feedback`, `/forget`.
- `retrieve`: `/search`, `/session/brief`,
  `/session/topic`, `/graph`.

## Depends on
`crate::auth` (extractors), `crate::error` (`{error, code}`), `cairn-store`, `cairn-core`.
