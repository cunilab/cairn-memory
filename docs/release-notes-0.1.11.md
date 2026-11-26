---
id: CM-REL-007
title: "Cairn Memory 0.1.11 release notes"
status: draft
---

# Cairn Memory 0.1.11 release notes

This release makes Cairn a Hermes Agent memory provider, requires Hermes v2026.4.23 or newer, and makes `cairn setup <agent> --remove` clean up the files and folders setup created. It has no database migrations and no server or web console changes apart from a dependency update.

## Fixes

- `cairn setup <agent> --remove` leaves no empty files behind. It deletes an instruction, plugin or host config file it changes to whitespace only, an empty JSON object or an empty YAML mapping, such as an instruction file setup created for the Cairn block, `~/.claude.json`, the Antigravity MCP, hook and settings files, Grok's Cairn hook file and `config.toml`, and Codex's `config.toml` once `codex mcp remove` empties it. Empty folders above a deleted file go too, up to the home folder. A file with any other key or content stays, and so does a file removal did not change.
- Hermes removal also deletes the bytecode Hermes's Python writes for Cairn's plugin (`plugins/cairn-memory/__pycache__/__init__.*.pyc`), so the empty plugin folder and an empty `plugins` folder are removed. A plugin folder holding any other file stays with that file.
- The web console's lockfile resolves `minimatch` 11.0.9 instead of 10.2.6 for the Dependabot alert, through `shadcn` 4.23.1 and `typescript-eslint` 8.74.0. `package.json` ranges are unchanged.

## Changes and token costs

| Area | Change | Agent token cost |
| --- | --- | --- |
| Hermes | `cairn setup hermes` makes Cairn the profile's memory provider for the CLI and for `--gateway` chat profiles: it sets `memory.provider: cairn-memory` in `config.yaml`. The provider runs the brief, prompt recall, the Stop and PreCompact checkpoints, SessionEnd and session switches (`/new`, `/reset`, `/resume`, `/branch` and compression). | Same brief and recall budgets as other agents. |
| Hermes | The provider offers the five memory tools and four skill tools natively through a new `cairn hook Tool` event, so setup removes the Cairn-managed `mcp_servers.cairn` entry. Gateway profiles get the chat tools instead: `memory_search`, `memory_save` and the four skill tools. Provider tools appear only while Hermes's `memory` toolset is enabled. | The same tool schemas as `tools/list`, offered by the provider instead of MCP. |
| Hermes | Hermes allows one external memory provider per profile. Setup saves the previous `memory.provider` value exactly, including an absent key or `none`, and `--remove` restores it while `memory.provider` is still `cairn-memory`; a provider chosen later is kept. | None. |
| Hermes | Gateway profiles keep their hashed chat routes, chat salt and 30-minute chat sessions. Hermes `/new` or `/reset` now ends the chat's Cairn session (Stop and SessionEnd) through Hermes's `on_session_finalize` hook, which gateway shutdown also fires; a reset sent during a running turn takes effect when that turn ends. The next message starts a new chat session with the chat instruction. Hermes evicting an idle agent keeps the chat session. | None. |
| Hermes | Hermes built-in memory (`memories/MEMORY.md`, `memories/USER.md`, `memory.memory_enabled`, `memory.user_profile_enabled`) is left unchanged and is not imported or mirrored. | None. |
| Hermes | Tool events stay plugin hooks, because providers have none: `post_tool_call` captures tool activity, looks up command warnings and recalls related memories for failed commands, and `transform_tool_result` attaches them to the terminal result. Hermes's turn-end `on_session_end` hook checkpoints failed or interrupted turns. The plugin manifest declares `kind: standalone` so Hermes loads both the hooks and the provider. | Unchanged. |
| Hermes | Cairn needs Hermes v2026.4.23 or newer. v2026.4.16 lacks `transform_tool_result` and the bound gateway session context, so command warnings and failed-command recall never reach the model and gateway chat memory is off. Setup, `cairn update` refresh and `cairn doctor` read `__release_date__` from the installer's `hermes-agent/hermes_cli/__init__.py` and warn, without failing, when Hermes is older or its version is unknown. They never run Hermes. | None. |
| Doctor | For Hermes, `cairn doctor` checks that the profile selects Cairn as its memory provider instead of checking for an MCP entry. | None. |
| CI | The Windows build job runs the update tests (`--version` and `update --check` against a local release listing) and `cairn doctor` with an empty home, which must print fail lines and exit 1. CI also runs the stubbed Hermes plugin and provider tests. | None. |

See [hook setup](reference/hook.md#setup), [Hermes gateway](reference/hook.md#hermes-gateway) and [Hermes provider tools](reference/cli-and-mcp.md#hermes-provider-tools) for details.

## Database migrations

None.

## Upgrade

1. No server or console changes are required; upgrading the server and console together as usual is fine.
2. On each machine run `cairn update`. It refreshes the managed integrations it finds, including the Hermes profile selected by `HERMES_HOME` (the default profile when it is unset).
3. With Hermes, update Hermes to v2026.4.23 or newer (`hermes update`), then run `cairn setup hermes` for every Hermes profile, with `HERMES_HOME=/path/to/profile` for profiles other than the default and `--gateway` for gateway profiles. `cairn update` does not reach other profiles. For each profile, setup:
   - removes the Cairn-managed `mcp_servers.cairn` entry, whose tools the provider now offers;
   - sets `memory.provider: cairn-memory`, replacing another external memory provider and saving it for `--remove`;
   - rewrites `plugins/cairn-memory/` with the provider and the new manifest.
4. Restart Hermes or the gateway, and use `/new` in gateway chats so Hermes loads the current tool schemas. Check with `hermes memory status`.
5. Run `cairn doctor` and address any failures and warnings. A profile still on the 0.1.10 setup reports that it has no Cairn memory provider.

## Known limits

- Hermes gateway chats were not tested live on Telegram or Discord. Setup, `cairn doctor`, `hermes memory status` and removal were checked against Hermes v0.23.4 (release date 2026.9.24) with a local server.
- The Windows CI check is new; `cairn doctor` with a configured device has not been run natively on Windows.
- A Hermes installed with pip has no installer checkout, so setup and `cairn doctor` report its version as unknown and warn even when it is new enough.
- `cairn setup hermes --remove` keeps the gateway chat salt, so a later setup finds the profile's chat memories, and leaves an empty `plugins.enabled` list. A Hermes `config.yaml` that setup created therefore stays.
- Hermes built-in memory import is not implemented.
