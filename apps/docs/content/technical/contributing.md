---
id: CM-DEV-001
title: "Development"
status: draft
description: "Develop Cairn with disposable stores, run the checks, and update the documentation."
---

# Development

Local development environment setup, workspace checks, test execution, and documentation rules.

The Cairn repository contains four Rust crates (`cairn-core`, `cairn-store`, `cairn-server`, `cairn-client`), the Next.js web console (`apps/web`), and the documentation site (`apps/docs`).

## Checks

Run the verification suite before opening a pull request:

```sh
# Rust formatting, compilation, and unit tests
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace

# Database migrations and integrity checks
python3 scripts/validate_phase0.py
python3 scripts/check_migrations.py
python3 scripts/test_check_migrations.py

# Acceptance smokes and web console checks
./scripts/smoke_mcp.sh
./scripts/smoke_web.sh
cd apps/web && npm run lint
```

- **Database tests**: Require a running PostgreSQL 17 + pgvector instance (`CAIRN_TEST_DATABASE_URL`).
- **Local stack**: Use `./scripts/dev_up.sh` to start Compose services and `./scripts/dev_down.sh` (or `--wipe` to reset data) to stop them.
- **Benchmarks**: Execute `scripts/bench.py` and `scripts/bench_write.py` to evaluate retrieval accuracy ([Benchmarks](../../../../bench/README.md)).

## Migrations

Preserve user state across all database revisions:

- **Additive only**: New columns must be nullable or provide defaults; write idempotent backfills for existing rows.
- **Destructive check**: `scripts/check_migrations.py` scans SQL files for `DROP`, `TRUNCATE`, and column deletions. Any destructive statement requires explicit entry in `migrations/destructive-allowlist.txt` with user approval.
- Applied migrations are strictly immutable.

## Documentation

Every documentation page under `apps/docs/content/` must follow Phase 0 rules:

- Valid YAML header containing unique `id`, `title`, and `status`.
- The top-level `# <Heading>` must match `title` exactly.
- All internal relative markdown links must resolve to existing files.
- Verified by `python3 scripts/validate_phase0.py`.
- Documentation site build and export tests:
  ```sh
  cd apps/docs && npm ci && npm run typecheck && npm run build && npm run check:export
  ```

See [Cairn Rust idioms](rust-idioms.md) and [Release process](release-process.md).
