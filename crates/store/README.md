# cairn-store

## What
All PostgreSQL access: the pool, the migrations in `migrations/`, and every query behind a
`Store` method. It applies the visibility rule in SQL and keeps each memory, link, embedding and job
change transactional. A `testing` feature adds `TestDb`, one schema per test.

## Entry points
- `Store::{connect, open, migrate}`; `StoreError`.
- Modules add `impl Store`: `account`, `device`, `project`, `memory` (save, get, list, feedback, forget,
  links), `capture`, `retrieve` (search, brief, topic notes, graph),
  `embed`, `consolidate`, `jobs`, `fixes`, `instructions`, `stats`, `link`, `repo`, `skill`, `transfer`, `view`, `usage`.
- `testing::TestDb` reads `CAIRN_TEST_DATABASE_URL`.

## Depends on
`cairn-core`, `sqlx` (PostgreSQL, pgvector).
