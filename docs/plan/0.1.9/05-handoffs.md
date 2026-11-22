---
id: CM-PLAN-019-05
title: "05 Handoffs that are not overwritten"
status: frozen
---

# 05 Handoffs that are not overwritten

## Goal

A short chat session no longer replaces a handoff that had files or open items. Next steps come only from explicit
`next:` or `todo:` lines, not from any prompt containing "should" or "need to". Changed files come from git when there
is git, and from the edit tools (inside the project root, plus a count of files outside it) when there is not.

## Out of scope

Handoff history/chain per branch (advisory D10). Cursor-based accumulation (D9 cursor). Changing handoff scope or TTL.

## Decisions it relies on

D-DATA, D-ANY, D-LLM, D-TOK.

## Design

- **Next steps** (`crates/core/src/process/distill.rs::handoff`, marker list l.296–306): next-step markers become exactly
  `next:` and `todo:` (case-insensitive, at line start after optional `- `/`* `). `should `, `need to `, `next step`,
  `follow up`, `remaining` are removed from the next-step list. The open-items marker list is unchanged.
- **No overwrite**: in `Store::distill_session` (`crates/store/src/capture.rs:279`, the transaction before the
  `DELETE FROM memories WHERE memory_id = $1` at l.353), read the current active handoff row for `handoff_id`. If it
  exists, is not expired, has a non-empty `fields.files` or `fields.open_questions`, and the new handoff has both empty,
  keep the old row and skip the write. `DistillReport` gets `handoff_kept: bool`; the audit event records it.
- **Changed files with git**: at `SessionStart` the hook stores the set of already-dirty paths
  (`git status --porcelain`, reusing `git::recent_files` / `git_run` in `crates/client/src/hook/git.rs`) in
  `SessionState.baseline_dirty` (cap 200). At each checkpoint (`Stop` when dirty, `PreCompact`, `SessionEnd`) the hook
  sends one `Edit` capture whose `context.files` = paths dirty now and not in the baseline, plus paths already in
  `SessionState.edited` (cap `HANDOFF_FILES_MAX`). This makes `rm`, `sed -i` and `git checkout` count. Skipped when the
  set is empty or equal to the last one sent (hash in state).
- **Changed files without git**: `git::repo_relative` (`git.rs:315`) uses the item-02 root instead of the git top. Paths
  outside the root are not stored; the hook counts them in `SessionState.outside_edits`, sent as
  `context.outside_files: <count>` on the checkpoint capture. The handoff summary becomes
  `"{n} files edited ({m} outside the project); {k} open items; last activity {date}."` (the parenthesis only when m>0).
- **Subagents/automation** never write handoffs (item 12).

## Allowed files

- `crates/core/src/process/distill.rs`
- `crates/store/src/capture.rs`
- `crates/client/src/hook/mod.rs`, `crates/client/src/hook/state.rs`, `crates/client/src/hook/git.rs`
- tests: unit tests in `distill.rs`, `crates/store/tests/processing.rs`, `crates/server/tests/hook.rs`

## Acceptance checks

1. A prompt "we should maybe adjust the roadmap, need to think" yields no next step.
2. A prompt line `next: wire the web report` yields exactly that next step; `TODO: fix flaky test` too.
3. Handoff A (2 files, 1 open item) then a chat-only session in the same checkout: A is still the active row, unchanged.
4. Handoff A then a session that edits files: the new handoff replaces A (today's behaviour).
5. A Bash `rm tracked.txt` in a git checkout appears in the handoff's `files`.
6. A file already dirty before the session and untouched is not listed.
7. In a non-git project folder, an edit inside the root is listed; an edit in `/tmp` is not listed and the summary says `(1 outside the project)`.

## Tests to write first

- `distill.rs::next_steps_need_explicit_marker` (1, 2)
- `processing.rs::chat_session_keeps_richer_handoff` (3, 4)
- `hook.rs::shell_deletes_count_as_changed_files` (5, 6)
- `hook.rs::no_git_edits_relative_to_project_root` (7)

## Data-safety notes

Reduces loss: the richer handoff is kept. The existing same-id replacement (`DELETE` + `INSERT` of a system-derived
handoff row) stays only for a handoff that is at least as rich; no other rows are deleted.

## Token cost

0 for the agent. One extra git call per checkpoint (client side).

## Risks

Large dirty trees: capped at 200 baseline / `HANDOFF_FILES_MAX` reported paths.

## Open questions

None.
