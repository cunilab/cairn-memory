---
id: CM-DOC-USERS-DAILY-USE
title: "Daily use"
status: accepted
description: "Save, find and correct useful decisions and resume work across devices."
---

# Daily use

## Save a decision

Give the agent a concrete title, summary and reason. Include the project context where the decision applies. The [quickstart](index.mdx) has a CLI example. With an agent, ask it to save the decision as soon as you have verified it. Add evidence such as a relevant source file.

## Find the full record

Ask the agent to search for the decision with `memory_search` or run `cairn search "disposable PostgreSQL schema"`. Use the returned ID in `memory_get` or `cairn get ID` to read the reason, evidence and links. Add `--detail` to read stored long-form detail for one ID.

## Correct what changed

Save the updated decision with `supersedes` set to the previous ID. Include a reason explaining the change. The old record remains searchable with a replaced status. Use [feedback](memory/trust.md) to record a wrong or helpful result.

## Resume on another device

Configure the other device with a token from the same account or have a teammate create their own. Open the shared checkout and run `cairn init`. [Project membership](memory/scopes.md) lets teammates see project-scoped decisions, while a personal handoff follows your account and checkout context. Ask the agent for the last handoff when the opening brief is insufficient.

## Capture controls

Hooks capture prompts, edits and non-read-only commands. Add `.cairnignore` to disable capture for a checkout. Assistant text is opt-in with `CAIRN_CAPTURE_ASSISTANT=1`. Use `CAIRN_SESSION_ORIGIN=automation` for automation, which can capture and distill without replacing a personal handoff or triggering the Stop reminder. [Hook protocol](../technical/hooks/index.md).
