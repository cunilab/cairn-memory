---
id: CM-PLAN-019-00
title: "00 Migration guard"
status: frozen
---

# 00 Migration guard

## Goal

CI fails when a migration under `migrations/` destroys data or schema: `DROP`, `DELETE FROM`, `TRUNCATE`, or
`ALTER TABLE ... DROP`. Known, approved cases are listed in an allowlist. Implemented first, so every later item is checked.

## Out of scope

Checking Rust code (`DELETE` in `crates/store/src/capture.rs` sweeps is runtime retention, already designed); checking
`refrences-project/`; rewriting old migrations.

## Decisions it relies on

D-DATA.

## Design

- New script `scripts/check_migrations.py` (stdlib only, like `validate_phase0.py`).
- Reads every `migrations/*.sql`. Strips `--` line comments and `/* */` blocks, then splits on `;` into statements.
- Flags a statement (case-insensitive, whitespace-normalised) matching any of:
  - `\bDROP\s+(TABLE|COLUMN|SCHEMA|DATABASE|VIEW|MATERIALIZED\s+VIEW|TYPE|INDEX|FUNCTION|TRIGGER|EXTENSION)\b`
  - `\bALTER\s+TABLE\b.*\bDROP\b` (covers `DROP COLUMN`, `DROP CONSTRAINT`, `DROP DEFAULT`, `DROP NOT NULL`)
  - `\bDELETE\s+FROM\b`
  - `\bTRUNCATE\b`
  - `ON DELETE CASCADE` / `ON DELETE SET NULL` in a `REFERENCES` clause is **not** flagged (it is a foreign-key rule, not a statement).
- Allowlist file `migrations/destructive-allowlist.txt`. One entry per line: `<file>|<normalised statement prefix>|<reason>`.
  `#` comments allowed. A flagged statement passes only when its file matches and the normalised statement starts with the prefix.
  An entry that matches nothing is itself an error (stale allowlist).
- Initial allowlist (the three existing cases, verified with `grep -niE "drop|delete|truncate" migrations/*.sql`):
  - `0003_retrieval.sql|DROP TABLE memory_embeddings|0.1.6 vectors moved to memories.embedding; embed jobs refill`
  - `0004_processing.sql|ALTER TABLE entities DROP CONSTRAINT entities_kind_check|widened CHECK, re-added in same migration`
  - `0004_processing.sql|ALTER TABLE jobs DROP CONSTRAINT jobs_kind_check|widened CHECK, re-added in same migration`
- Output: one line per violation `migrations/<file>: <statement first 80 chars>`, then a message pointing at this spec; exit 1.
- CI: new step in the `Rust format and clippy` job of `.github/workflows/ci.yml`, next to the other grep guards:
  `python3 scripts/check_migrations.py` and `python3 scripts/test_check_migrations.py`.
- Adding an allowlist line is a user decision: the PR description must quote the user's approval.

## Allowed files

- `scripts/check_migrations.py` (new)
- `scripts/test_check_migrations.py` (new)
- `migrations/destructive-allowlist.txt` (new)
- `.github/workflows/ci.yml` (one step added)

## Acceptance checks

1. On the current tree the script exits 0 and prints `migrations: 6 files, 3 allowlisted`.
2. A temp migration with `DROP TABLE x;` exits 1 and names the file.
3. `ALTER TABLE memories DROP COLUMN reason;` exits 1.
4. `DELETE FROM memories;` and `TRUNCATE observations;` each exit 1.
5. `-- DROP TABLE x` (comment) and `REFERENCES memories ON DELETE CASCADE` exit 0.
6. `ALTER TABLE memories ADD COLUMN rank text NOT NULL DEFAULT 'normal';` exits 0.
7. An allowlist entry that matches no statement exits 1 ("stale allowlist entry").
8. CI runs both scripts in the lint job.

## Tests to write first

`scripts/test_check_migrations.py` (runs the checker on temp dirs, like `test_download_embedding_model.py`):
- `test_current_tree_passes` — check 1.
- `test_drop_table_fails`, `test_alter_drop_column_fails`, `test_delete_and_truncate_fail` — checks 2–4.
- `test_comments_and_fk_rules_pass` — check 5.
- `test_additive_passes` — check 6.
- `test_stale_allowlist_fails` — check 7.

## Data-safety notes

This is the enforcement point for D-DATA. It cannot see data deleted by Rust code; items that touch runtime deletes
(item 05) state their own rule.

## Token cost

None (CI only).

## Risks

Regex SQL parsing misses exotic syntax (`DO $$ ... $$` blocks). Mitigation: `$$` bodies are scanned too, not skipped.

## Open questions

None.
