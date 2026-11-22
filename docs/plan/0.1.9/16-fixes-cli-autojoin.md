---
id: CM-PLAN-019-16
title: "16 Fixes: device fixes, cairn fixes, audited auto-join"
status: frozen
---

# 16 Fixes: device fixes, cairn fixes, audited auto-join

## Goal

Computer fixes the admin applied (item 15 sets `queued`) run on the one device they name, from an interactive `cairn`
command. A person becomes a project member automatically when they use Cairn in a folder whose
`.cairn/project.json` names that project, and every such join is audited. A git remote alone never joins anyone.

## Out of scope

Listing or applying fixes for other devices or people from the terminal (admin list stays web-only). Running any fix
from the hook or MCP. Shell strings or server-sent paths. Committing `.cairn/project.json`.

## Decisions it relies on

D-ANY, D-DATA, D-TOK. User: choice B; auto-join only through `.cairn/project.json`, never via the remote string; audit
the join; web-only tokens, CLI only for agents and setup.

## Design

- **Auto-join (already mostly there)**: `context_project` (`crates/store/src/memory.rs:232`) joins through
  `join_member` (`crates/store/src/project.rs:83`) when the context carries a `project_id`; the hook fills it only from
  `.cairn/project.json` (`crates/client/src/hook/git.rs:120`). `resolve_project`'s repository fallback attaches only
  existing members. Change: `join_member` reports whether the `INSERT` added a row; when it did, write audit
  `project_auto_joined` (`audit_tx`, `crates/store/src/account.rs:343`) with project id, person, device and
  `source: "project_file"`. `cairn init`'s `join_project` keeps its own path and gets the same audit when a row is added.
- **Device endpoint** (`crates/server/src/routes/device.rs`, device token): `GET /cairn-memory/device/fixes` returns
  only `queued` fixes whose `device_id` is the caller's; `POST /cairn-memory/device/fixes/{id}` with
  `{"result":"applied"|"skipped","reason"}` (reason is a closed enum: `already_current`, `file_exists`,
  `outside_checkout`, `not_this_repo`, `failed`). Applied → `done`, skipped `already_current` → `resolved`, other
  skips → `failed` with the reason; audit `fix_done` or `fix_failed`.
- **Client** `crates/client/src/fixes.rs` (new), closed actions:
  - `suggest_setup` (F4): re-run existing setup for the agent this device already configured (managed blocks only).
  - `run_update` (F5): call the existing updater (`crates/client/src/update.rs`) only if the compiled version is still
    older than `action.target`; else `already_current`.
  - `write_project_file` (F6): `project::write` (`crates/client/src/project.rs:157`) only when the current folder's git
    top is the repository named in the fix, the file is absent, and the path stays inside the top after symlinks.
- **When it runs**: any interactive `cairn` command in `crates/client/src/bin/cairn.rs` first fetches and applies this
  device's queued fixes, printing one line each ("Applied fx_…: wrote .cairn/project.json"). `cairn fixes` lists them;
  `cairn fixes apply [fx_id]` applies now. The hook and MCP never call this code and never print.
- `docs/user-guide.md`: `cairn fixes`, and that a committed project file joins whoever uses it (audited).

## Allowed files

- `crates/store/src/project.rs`, `crates/store/src/memory.rs` (`context_project` audit only), `crates/store/src/fixes.rs`
- `crates/server/src/routes/device.rs`, `crates/server/src/routes/mod.rs`
- `crates/client/src/fixes.rs` (new), `crates/client/src/lib.rs` (module line), `crates/client/src/bin/cairn.rs`
- `crates/client/src/project.rs` (call only), `crates/client/src/update.rs` (call only), `crates/client/src/setup/mod.rs` (call only)
- `docs/user-guide.md`
- tests: `crates/store/tests/project.rs`, `crates/server/tests/project_model.rs`, `crates/client/tests/fixes.rs` (new)

## Acceptance checks

1. Capture with a project file id the person is not a member of → member + exactly one `project_auto_joined` audit;
   a second capture adds no audit.
2. Capture with only a matching git remote and no project file → no membership change, no audit.
3. Device B never receives or applies device A's fix (403/empty).
4. `run_update` skipped as `already_current` when current; never runs from the hook or MCP paths (test calls both).
5. `write_project_file` refuses an existing file, a symlink escape, and a different repository.
6. Interactive command applies queued fixes first and prints one line each; hook stdout unchanged.
7. Unknown action names or extra fields from the server are rejected, not executed.

## Tests to write first

- `store/tests/project.rs::auto_join_audited_once` (1), `::remote_alone_never_joins` (2)
- `server/tests/project_model.rs::device_fixes_scoped` (3)
- `client/tests/fixes.rs::update_guarded` (4), `::project_file_guards` (5), `::apply_before_command` (6), `::closed_actions` (7)

## Data-safety notes

No data removed. Joins are additive and audited. `write_project_file` never overwrites.

## Token cost

0 agent tokens: hook and MCP unchanged; output only on interactive `cairn` commands.

## Risks

Anyone with a checkout holding the project file joins the project; this is the user's chosen trust rule and the
audit makes it visible on Activity. An update triggered from a command could surprise; it only runs after admin Apply.

## Open questions

None.
