---
id: CM-DOC-USERS-DAILY-USE
title: "Daily use"
status: accepted
description: "Save, find and correct useful decisions and resume work across devices."
---

# Daily use

## Save a decision

Instruct your agent to save verified findings immediately:

- **What to provide**: Title, summary, and explicit reason. Add context, alternatives, or source evidence where relevant.
- **CLI equivalent**:
  ```sh
  cairn save '{"kind":"decision","title":"Title","summary":"Summary","reason":"Why"}'
  ```
- **Rule of thumb**: Save only verified, reusable decisions, rules, gotchas, or procedures.

## Find the full record

Retrieve context explicitly whenever automatic recall is insufficient:

- **Search**: Run `cairn search "query"` or instruct the agent to call `memory_search`.
- **Read record**: Run `cairn get <id>` or pass `detail: true` (`cairn get <id> --detail`) for full technical details.

## Correct what changed

Keep memory accurate when requirements or architectural choices evolve:

- **[Supersede](../glossary.md#superseded)**: Save the revised decision with `supersedes: "<old_id>"` and a clear reason for the change.
- **History preserved**: The superseded record remains searchable with a `superseded` status.
- **Feedback**: Call `memory_feedback` with `helpful` or `wrong` to adjust confidence rankings ([Trust and corrections](memory/trust.md)).

## Resume on another device

Seamlessly continue development across machines or teammates:

- **Configure device**: Add a new token under **Settings → Devices** or join from a teammate account.
- **Attach repo**: Open the repository checkout and run `cairn init`.
- **[Handoffs](../glossary.md#handoff)**: Project members automatically receive project-scoped decisions. Personal handoffs track your account and checkout context. Ask your agent for the last handoff if the brief is incomplete.

## Capture controls

Control how Cairn records development activity:

- **[Automatic capture](../glossary.md#capture)**: Hooks capture prompts, code edits, and non-read-only terminal commands.
- **Opt out**: Add a `.cairnignore` file in a checkout root to disable capture entirely for that directory.
- **Assistant text**: Disabled by default; enable with `CAIRN_CAPTURE_ASSISTANT=1`. Without it, only the `Next:` and `Open:` lines of the agent's final message are kept, for the handoff.
- **Automated sessions**: Set `CAIRN_SESSION_ORIGIN=automation` in CI or batch scripts to capture and distill without overriding personal handoffs or triggering Stop reminders ([Hook protocol](../technical/hooks/index.md)).
