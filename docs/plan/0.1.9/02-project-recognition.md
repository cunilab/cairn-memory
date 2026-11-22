---
id: CM-PLAN-019-02
title: "02 Project recognition everywhere"
status: frozen
---

# 02 Project recognition everywhere

## Goal

A session attaches to the right project whether or not the folder is a git repository, and whether or not
`.cairn/project.json` is present in this checkout. When no project can be found, capture stays personal and the agent
gets a one-line hint. `cairn init` works without git and suggests committing the file; it never commits.

## Out of scope

Setting `default_branch` (advisory J5). Creating projects automatically. Running `git add`/`git commit` for the user.

## Decisions it relies on

D-ANY, D-TOK, D-DATA.

## Design

Order of resolution (client, hook and CLI share one function):

1. **Root.** `git rev-parse --show-toplevel` if inside git (`crates/client/src/project.rs::toplevel`,
   `crates/client/src/hook/git.rs::top_level`). Without git: the nearest ancestor of `cwd` (inclusive, stopping at `$HOME`
   or `/`) that contains `.cairn/project.json`. New `project::find_root(cwd) -> Option<PathBuf>`.
2. **File.** `.cairn/project.json` at that root (`project::read_at`, `git.rs::project_id`) → project id. Unchanged rules.
3. **Git-remote fallback** (file missing, git present): the client sends `repository` as today (`git.rs::repository_name`);
   the server resolves it. In `Store::save_from` trust path (`crates/store/src/memory.rs:176`, membership check near l.212)
   and in capture (`Store::capture_from`, `crates/store/src/capture.rs:45`): when `project_id` is absent and
   `repository` is set and not `local:`, look up projects with that `repository` where the person is a member.
   Exactly one match → use it. Zero or several → personal. The `/observe` reply's `project_attachment.status` gains
   `attached_by_repository`.
4. **No project** → personal. `Hook::start` (`hook/mod.rs:277`) appends one hint line instead of the session
   instruction: `Cairn: no project here (personal memory). Run cairn init to share with a project.` Once per session;
   off when `nudge` is false or `CAIRN_NO_NUDGE=1`; not printed when the server is unreachable. Paths the hook
   already skips (`git::ignored`) get nothing.
- `Hook::nudging` (`mod.rs:327`) uses the resolved project (file or repository fallback) rather than the file only; the
  brief reply carries the resolved `project_id`, which the hook stores in `SessionState` (`state.rs`, new field
  `project_id: Option<String>`).
- **`cairn init` without git** (`crates/client/src/bin/cairn.rs:167`): `top = toplevel(cwd).unwrap_or(cwd)`.
  Repository for a non-git folder: `local:<folder name>` (the form `distill.rs` already strips). After writing the file:
  - inside git and `git ls-files --error-unmatch .cairn/project.json` fails → print
    `Commit .cairn/project.json so clones and worktrees attach: git add .cairn/project.json && git commit`;
  - outside git → print `Keep .cairn/project.json with this folder; copies of the folder attach to the same project.`
  - never run `git add`/`git commit`.

## Allowed files

- `crates/client/src/project.rs`, `crates/client/src/project/tests.rs`
- `crates/client/src/hook/git.rs`, `crates/client/src/hook/mod.rs`, `crates/client/src/hook/state.rs`
- `crates/client/src/bin/cairn.rs`
- `crates/store/src/capture.rs`, `crates/store/src/memory.rs`, `crates/server/src/routes/capture.rs`
- tests: `crates/server/tests/hook.rs`, `crates/server/tests/project_model.rs`, `crates/store/tests/capture.rs`

## Acceptance checks

1. Non-git folder with `.cairn/project.json` in a parent: hook captures attach to that project.
2. Git checkout without the file, origin matches exactly one member project: captures attach, status `attached_by_repository`.
3. Same, but the person is a member of two projects with that repository: personal.
4. Non-member: personal (no project leak).
5. No project: session start prints the hint once (≤ 90 bytes); none when `CAIRN_NO_NUDGE=1`.
6. `cairn init` in a non-git folder succeeds, writes the file with repository `local:<name>`, prints the keep-folder line.
7. `cairn init` in git with the file untracked prints the commit suggestion; `git status` shows no new commit and no staged file.
8. Existing behaviour with a committed file is unchanged (existing tests pass).

## Tests to write first

- `project/tests.rs::find_root_walks_up_without_git` (1)
- `capture.rs::repository_fallback_needs_exactly_one_membership` (2, 3, 4)
- `hook.rs::no_project_start_prints_hint_once` (5)
- `hook.rs::init_without_git_writes_local_repository` (6)
- `hook.rs::init_suggests_commit_and_never_commits` (7)

## Data-safety notes

No schema change. The fallback only attaches new captures; it never moves existing rows.

## Token cost

Hint line ~22 tokens once per session, only when there is no project (it replaces the 39-token session instruction).

## Risks

A person in two projects with the same remote (fork vs upstream) gets personal; that is the safe default.

## Open questions

None.
