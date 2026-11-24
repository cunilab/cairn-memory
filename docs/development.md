---
id: CM-DEV-001
title: "Development"
status: draft
---

# Development

The workspace contains `crates/core`, `crates/store`, `crates/server`, and `crates/client`. The console is in `apps/web`. Rust conventions are in [Rust idioms](RUST_IDIOMS.md).

## Checks

```sh
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
python3 scripts/validate_phase0.py
python3 scripts/check_migrations.py
python3 scripts/test_check_migrations.py
./scripts/smoke_mcp.sh
./scripts/smoke_web.sh
cd apps/web && npm run lint
```

Database tests need a disposable PostgreSQL instance with pgvector and `CAIRN_TEST_DATABASE_URL`. CI uses PostgreSQL 17 and requires the database rather than skipping tests. The smoke scripts isolate their data. For local development `./scripts/dev_up.sh` starts the Compose stack and `./scripts/dev_down.sh` stops it while keeping data. `./scripts/dev_down.sh --wipe` removes its database volume.

`scripts/bench.py` measures search and injection; `scripts/bench_write.py` checks capture and session output. See [the benchmark](../bench/README.md).

## Migrations

Preserve user data. New migrations should be additive and include an idempotent backfill when existing rows need new values. Test backfills on a database containing existing rows. Never edit applied migrations.

`scripts/check_migrations.py` scans SQL for destructive statements such as `DROP`, `ALTER TABLE ... DROP`, `DELETE FROM` and `TRUNCATE`. Foreign-key `ON DELETE` clauses are not destructive statements. The checker skips comments and requires each matching statement to have an entry in `migrations/destructive-allowlist.txt`; stale entries fail. A new allowlist entry requires explicit user approval, quoted in the PR description. The checker is not a substitute for reviewing runtime deletion or all SQL syntax.

## Documentation

Every Markdown file under `docs/` starts with YAML fields `id`, `title`, and `status`, has a unique ID, and has a first heading matching its title. Relative links must resolve. The root README needs no YAML header. `scripts/validate_phase0.py` checks these rules. Keep one source of truth per topic and link to it from other pages.
