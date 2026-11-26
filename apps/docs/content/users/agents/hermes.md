---
id: CM-DOC-USERS-AGENTS-HERMES
title: "Hermes Agent"
status: accepted
description: "Set up a Hermes profile to use Cairn as its memory provider."
---

# Hermes Agent

## Check the Hermes version

Cairn needs Hermes v2026.4.23 or newer. v2026.4.16 can load Cairn as a provider, but it lacks the `transform_tool_result` hook, so command warnings and failed-command recall never reach the model, and it lacks the bound gateway session context, so gateway chat memory is off. Earlier releases cannot load a provider from the profile at all. Setup and `cairn doctor` read `__release_date__` from the installer's `hermes-agent/hermes_cli/__init__.py` next to the profile or its profiles folder, and warn when it is older or cannot be found; they never run Hermes. [transform_tool_result added](https://github.com/NousResearch/hermes-agent/commit/04068c5891ada4052088ed1febabd9384c87c649), [v2026.4.23](https://github.com/NousResearch/hermes-agent/releases/tag/v2026.4.23)

## Connect a profile

Supply the device token on stdin. These commands target the default profile.

```sh
cairn setup https://your-server.example --agent hermes
cairn doctor
```

Set `HERMES_HOME` to target a different profile. After restarting Hermes, check provider and plugin discovery.

```sh
hermes memory status
hermes plugins list
```

Hermes Agent setup makes Cairn the profile's memory provider: it sets `memory.provider: cairn-memory` in `config.yaml`, enables `plugins/cairn-memory/` through `plugins.enabled` and adds the marked block to `SOUL.md`. Hermes allows one external memory provider per profile, so setup replaces another one and saves its exact previous value, including an absent key or `none`. `cairn setup hermes --remove` restores that value while `memory.provider` is still `cairn-memory`; a provider chosen later is kept. Setup removes the Cairn-managed `mcp_servers.cairn` entry from earlier versions, because the provider offers the same tools natively ([Hermes provider tools](../reference/mcp.md#hermes-provider-tools)); removal does not add it back. Hermes built-in memory (`MEMORY.md`, `USER.md`, `memory.memory_enabled`, `memory.user_profile_enabled`) is left unchanged, and Cairn does not mirror built-in memory writes. Run setup with `HERMES_HOME` pointing at your profile to target a profile other than the default. The YAML rewrite preserves unrelated settings and may change comments and formatting. Use `hermes memory status` and `hermes plugins list` to check discovery after restarting Hermes. [Hermes profile paths](https://github.com/NousResearch/hermes-agent/blob/f85f33bf534d0a4ac88c348d34a4c9ddb424dcb7/hermes_constants.py), [one external provider](https://github.com/NousResearch/hermes-agent/blob/f85f33bf534d0a4ac88c348d34a4c9ddb424dcb7/agent/memory_manager.py#L285-L296), [provider interface](https://github.com/NousResearch/hermes-agent/blob/f85f33bf534d0a4ac88c348d34a4c9ddb424dcb7/agent/memory_provider.py#L84-L267), [Hermes plugins](https://hermes-agent.nousresearch.com/docs/developer-guide/plugins), [Hermes context files](https://hermes-agent.nousresearch.com/docs/user-guide/features/context-files)

## Tool availability

Keep the platform's Hermes `memory` toolset enabled to access the [provider tools](../reference/mcp.md#hermes-provider-tools). For chat platforms use a [dedicated gateway profile](hermes-gateway.md).
