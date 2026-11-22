---
id: CM-PLAN-019-04
title: "04 Session digest note"
status: frozen
---

# 04 Session digest note

## Goal

Every session that changed files leaves one free, searchable digest note: branch, date, files touched, test commands
with pass/fail. It is project-visible, starts at rank `candidate`, never appears in the brief, and expires unless used.

## Out of scope

Prompt text in the digest (privacy and noise). Digests for sessions without changed files. A new memory kind.

## Decisions it relies on

D-DIGEST, D-RANK, D-LLM, D-DATA, D-TOK.

## Design

- Built in `crates/core/src/process/distill.rs` by a new `fn digest(observations) -> Option<SaveInput>`, returned in
  `Distilled` (new field `digest: Option<SaveInput>`). No LLM.
- Only when the session has at least one changed file (the `Edit` captures, including the item-05 checkpoint capture).
- Kind `note` (existing kind, no CHECK change; `kind.rs` gives notes the 90-day TTL `NOTE_TTL_SECONDS`).
- Title: `Session {date} on {branch|repository basename}` (≤ 80 chars).
- Summary (≤ 240 chars): `Changed {files joined with ", ", first 6, "+N more"}. Tests: {cmd pass|fail, first 3}.`
  Test commands are those `is_test_command` (`distill.rs:221`) recognises, with the last result per command.
- Reason: `Derived from the captures of one session.`; evidence: up to 20 paths and commands.
- Saved by `Store::distill_session` (`crates/store/src/capture.rs:279`) with `Origin::System`, scope `project` when the
  session has a project, else `personal`, rank `candidate` (item 01).
- Search: normal ranking with the existing `Kind::Note` adjust (-0.05) plus the candidate adjust (-0.10). Brief: excluded
  (candidate). Promotion to `normal` when fetched (item 01), which also keeps it from being treated as unused.
- Dedup: the identity hash includes title + summary, so a retried distill of the same session does not duplicate.
- Automated/subagent sessions (item 12) still get a digest (it is evidence, not a handoff).

## Allowed files

- `crates/core/src/process/distill.rs`
- `crates/store/src/capture.rs`
- tests: unit tests in `distill.rs`, `crates/store/tests/processing.rs`, `crates/store/tests/retrieve.rs`

## Acceptance checks

1. Session with edits to 2 files and `cargo test` failing then passing → one note titled `Session YYYY-MM-DD on <branch>` whose summary names both files and `cargo test pass`.
2. Session with no changed files → no digest.
3. Digest is `scope='project'` in a project session, `rank='candidate'`, `origin='system'`, `expires_at` = created + 90 days.
4. Digest is never in `/session/brief`; `memory_search "LedControls removed"` finds the matching digest.
5. Distilling the same session twice leaves one digest.
6. Summary never contains prompt text.

## Tests to write first

- `distill.rs::digest_lists_files_and_test_results` (1, 6)
- `distill.rs::no_digest_without_changed_files` (2)
- `processing.rs::digest_is_project_candidate_with_note_ttl` (3, 5)
- `retrieve.rs::digest_searchable_not_in_brief` (4)

## Data-safety notes

Adds rows only. TTL expiry of unused notes is the existing sweeper behaviour for `note` (`capture.rs::sweep`), unchanged.

## Token cost

0 injected (not in the brief). A search hit costs one card (~60–100 tokens) like any note.

## Risks

Search noise; mitigated by the two negative adjusts and the gate (a digest still has to pass it).

## Open questions

None.
