---
id: CM-PLAN-019-13
title: "13 Fixes: recording"
status: frozen
---

# 13 Fixes: recording

## Goal

One additive migration and the hook changes that record the data the Fixes detectors (item 14) read: whether the
Stop save question was asked and followed, which search phrases missed, hook errors, and the `fixes` table itself.
Nothing here is shown to agents; recording costs 0 agent tokens.

## Out of scope

Detectors (14), the admin page and Apply (15), the `cairn fixes` command and auto-join audit (16). Any LLM.

## Decisions it relies on

D-LLM, D-DATA, D-TOK. User decisions for Fixes: choice B (nothing applied without the admin's click), no Undo,
search log kept 14 days (raw log expiry is allowed; memories are never deleted), 0 extra tokens in sessions.

## Design

- Migration `0011_fixes.sql` (after item 08's `0010`). Only `CREATE TABLE`, `ADD COLUMN`, `CREATE INDEX`; existing
  rows keep NULL, meaning "not recorded":
  - `CREATE TABLE fixes (fix_id text PRIMARY KEY, detector text NOT NULL, fingerprint text NOT NULL, subject_kind text,
    subject_id text, project_id text, device_id text, person_id text, fix_type text NOT NULL CHECK (fix_type IN
    ('server','computer','agent','report')), status text NOT NULL CHECK (status IN ('pending','applying','queued',
    'waiting','done','failed','resolved','ignored','expired')), count int NOT NULL DEFAULT 1, first_seen_at timestamptz
    NOT NULL, last_seen_at timestamptz NOT NULL, quiet_seen_at timestamptz, ignored_until timestamptz, evidence jsonb
    NOT NULL DEFAULT '{}', action jsonb, before jsonb, applied_by text, applied_at timestamptz, waiting_since
    timestamptz, done_at timestamptz, proof_id text, error text)`;
    unique index on `(detector, fingerprint) WHERE status IN ('pending','applying','queued','waiting')`.
    `before` is an audit snapshot of the old value, not an undo (there is no Undo).
  - `CREATE TABLE lookup_log (id bigserial PRIMARY KEY, person_id text NOT NULL, device_id text, session_id text,
    at timestamptz NOT NULL DEFAULT now(), kind text NOT NULL CHECK (kind IN ('search','get')), query text CHECK
    (char_length(query) <= 120), result_ids text[] NOT NULL DEFAULT '{}', hit boolean)` + index `(session_id, at)`.
  - `sessions ADD COLUMN nudge_asked boolean, ADD COLUMN nudge_saved boolean`.
  - `devices ADD COLUMN hook_error_at timestamptz, ADD COLUMN hook_error_class text CHECK (hook_error_class IN
    ('network','auth','server','parse')), ADD COLUMN outbox_depth integer`.
  - `memories ADD COLUMN faded_at timestamptz, ADD COLUMN edited_by text` (`edited_by` = admin person id when an
    admin rewrote the memory, item 15; a new column instead of a new `rank` value so no CHECK is dropped).
  Register it in the migration list in `crates/store/src/lib.rs`.
- **Nudge flags**: the hook adds `nudge_asked` / `nudge_saved` (booleans only) to the session-end post, from
  `SessionState.nudged` / `saved` (`crates/client/src/hook/state.rs`), before it deletes local state. Automated or
  subagent sessions (item 12) send nothing. The server stores them in `upsert_session` (`crates/store/src/capture.rs`).
- **Lookup log**: the `PostToolUse` matcher written by setup (`crates/client/src/setup/mod.rs`) adds
  `mcp__cairn__memory_search` and `mcp__cairn__memory_get`. The hook posts `{kind, query, result_ids, hit}`: query is
  clipped to 120 chars after `redact_sensitive_content` (`crates/core/src/capture/redact.rs:82`), at most 20 ids. The
  hook prints nothing back. New store fn in `crates/store/src/fixes.rs` (new) inserts the row.
- **Hook errors**: the outbox (`crates/client/src/hook/outbox.rs`) remembers the last failure class and sends it with
  `outbox_depth` on the next successful post; no error text. Stored on `devices` (`crates/store/src/device.rs`).
- **Sweep**: `Store::sweep` (`crates/store/src/capture.rs:455`) deletes `lookup_log` rows older than 14 days. This is
  log expiry, not memory data. `fixes` rows are never deleted.
- `docs/contracts/hook-v1.md` documents the new optional fields.

## Allowed files

- `migrations/0011_fixes.sql` (new), `crates/store/src/lib.rs`, `crates/store/src/fixes.rs` (new)
- `crates/store/src/capture.rs`, `crates/store/src/device.rs`
- `crates/server/src/routes/capture.rs`, `crates/server/src/routes/device.rs`
- `crates/client/src/hook/{mod.rs,state.rs,outbox.rs,event.rs}`, `crates/client/src/setup/mod.rs` (matcher only)
- `docs/contracts/hook-v1.md`
- tests: `crates/store/tests/migrate.rs`, `crates/store/tests/capture.rs`, `crates/client/tests/hook.rs`, `crates/server/tests/hook.rs`

## Acceptance checks

1. `scripts/check_migrations.py` passes with no new `destructive-allowlist.txt` line.
2. Migration on a database with rows in every table leaves row counts and `memory_id`s unchanged.
3. Hook stdout bytes are identical before and after for search, get, Stop and SessionEnd events.
4. `lookup_log.query` ≤ 120 chars and redacted (a fixture with an API key stores no key).
5. Rows older than 14 days are swept; younger ones stay; no other table loses rows.
6. Automated sessions store NULL `nudge_asked`.
7. Device hook error class and outbox depth arrive on the next successful post.

## Tests to write first

- `migrate.rs::fixes_migration_additive` (1, 2)
- `client/tests/hook.rs::lookup_recorded_silently` (3, 4), `::nudge_flags_sent` (6)
- `store/tests/capture.rs::lookup_log_swept_after_14_days` (5)
- `server/tests/hook.rs::hook_error_reported` (7)

## Data-safety notes

Additive only. The only delete is the 14-day `lookup_log` expiry, allowed by the user; memories are never deleted.

## Token cost

0 agent tokens: no hook output, no schema change. Storage ≈ 0.4 KB per lookup.

## Risks

Old setups lack the widened matcher until `cairn setup` reruns (F4/F5 in item 14 surface them). Search phrases are
personal; only admins see them (item 15) and secrets are redacted before storage.

## Open questions

None.
