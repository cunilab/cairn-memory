---
id: CM-REF-002
title: "Agent hooks"
status: draft
description: "Shared capture events, injection budgets, redaction and retry behavior."
---

# Agent hooks

[Hooks](../../glossary.md#hook) send selected session events to Cairn and can deliver relevant memories back to the agent.

`cairn hook <Event>` consumes JSON payloads from agent host events via stdin, redacts sensitive tokens, and delivers contextual memories or queues observations for distillation.

## Events

| Event | Execution action | Context injected |
| --- | --- | --- |
| `SessionStart` | Initializes session and repository context | Session brief (L0 pins + L1 essentials) up to 1,800 bytes |
| `UserPromptSubmit` | Captures user prompt text | Relevant preferences, project rules, and matched skill card |
| `PreToolUse` | Command warning lookup before execution | Up to 2 gotchas or procedures for matching commands |
| `PostToolUse` | Captures file edits and shell commands | File topic notes (up to 4 cards); recognizes Cairn saves |
| `PostToolUseFailure` | Intercepts failed tool results (Claude Code) | Related gotchas and procedures from error output |
| `Stop` | Checkpoints session work | Evaluates Stop reminder to save reusable notes |
| `PreCompact` | Checkpoints active session before context compaction | None |
| `SessionEnd` | Concludes session and enqueues background distillation | None |

- **Automation**: Automated sessions (`CAIRN_SESSION_ORIGIN=automation`) capture and distill without prompting the Stop reminder or overwriting personal handoffs.
- **Assistant text**: Captured only when explicitly opted in via `CAIRN_CAPTURE_ASSISTANT=1`. Without it, `Stop` sends only the `Next:` and `Open:` lines of the final message (redacted, at most 10 lines and 1 KiB) for the handoff.

## Reminders (agent nudges)

- **Session start nudge**: Instructs agents to treat Cairn as their sole memory service.
- **Stop reminder**: Runs once per eligible session after file edits or corrections, asking to save up to 3 reusable notes and, if work is unfinished, to end with a `Next:` line. Suppressed if notes were already saved. Hermes coding sessions get it with the next prompt's recall instead of at Stop.
- **Nudge overrides**: Project instructions override global instructions, which override default prompts. Set Stop text to `off` to disable. Set `nudge: false` in config or `CAIRN_NO_NUDGE=1` to disable all nudges.

## Injection budget

- **Session brief**: Up to 1,800 bytes (900 B L0 + 900 B L1) for modern clients (`extended_brief: true`); capped at 1,200 bytes for legacy clients.
- **Session ledger**: Up to 8 L2 cards shared across file topics, command warnings, error recall, and prompts per session ID; session brief cards deduplicate without spending the L2 budget.
- **File topics**: Evaluates up to 16 file topics; injects up to 4 topic cards of 300 bytes each.

## Redaction

The hook and server execute 18 ordered regex patterns replacing secrets with `[REDACTED]` prior to persistence:

1. PEM private key blocks (`-----BEGIN ... PRIVATE KEY-----`)
2. Bearer, Basic, and Token Authorization headers
3. Userinfo in connection URLs (`scheme://user:pass@host`)
4. Password and token CLI arguments (`--password`, `--token`, `--api-key`)
5. Configuration key-value assignments (`DB_PASSWORD=...`, `"token": "..."`)
6. Database client flags (`mysql -p...`)
7. cURL credentials (`curl -u user:pass`)
8. GitHub tokens (`ghp_`, `gho_`, `github_pat_`)
9. AWS access key IDs (`AKIA...`, `ASIA...`)
10. Slack tokens (`xoxb-`, `xoxp-`)
11. Anthropic and OpenAI keys (`sk-ant-`, `sk-proj-`)
12. Stripe API keys (`sk_live_`, `sk_test_`)
13. npm package tokens (`npm_`)
14. GitLab tokens (`glpat-`)
15. Google API keys (`AIza...`)
16. JSON Web Tokens (`eyJ...`)
17. Cairn device tokens (`cdt_...`)
18. Legacy Hermes owner IDs

## Outbox

A local disk [outbox](../../glossary.md#outbox) (`outbox.json` in config dir, or `CAIRN_OUTBOX`) queues transient capture failures:

- **Binding**: Bound strictly to the server URL and device token hash.
- **Capacity**: Buffers up to 256 events (oldest dropped when full). Retries up to 10 times with backoff.
- **Replay**: Successfully delivered hook events automatically flush and replay queued records.

## Setup

See [Agent setup guide](../../users/agents/index.md#setup) for configuration and token registration.

## Command recall by host

| Host | Warning lookup (PreToolUse) | Failed-command recall (PostToolUse) |
| --- | --- | --- |
| **Claude Code** | Bash `PreToolUse` returns `additionalContext`. | `PostToolUseFailure` attaches error context to output. |
| **Codex** | Bash `PreToolUse` returns `additionalContext`. | `PostToolUse` attaches context if nonzero exit is reported. |
| **Grok Build** | Bash `PreToolUse` runs before execution; delivered with batch. | `PostToolUse` accepts `additionalContext` for nonzero exits. |
| **Antigravity CLI** | `run_command` `PreToolUse` returns `allow` with warning reason. | `PostToolUse` queues cards for next `PreInvocation.injectSteps`. |
| **OpenCode V2** | `execute.before` queues warning; delivered via next `context` hook. | `execute.after` queues error cards for next context hook. |
| **Hermes Agent** | Policy event has no advisory channel; evaluated in `post_tool_call`. | `transform_tool_result` attaches recall to terminal output. |

## Prompt recall by host

| Host | Prompt injection mechanism |
| --- | --- |
| **Claude Code & Codex** | `UserPromptSubmit` returns `hookSpecificOutput.additionalContext`. |
| **Grok Build** | `UserPromptSubmit` captures prompt; defers card injection to next `PostToolUse`. |
| **Antigravity CLI** | `PreInvocation` injects cards via `injectSteps` (deduplicates within execution). |
| **OpenCode V2** | Prompt hook queues matched cards for delivery in the next `session.hook("context")`. |
| **Hermes Agent** | Memory provider's `on_turn_start` evaluates prompt; `prefetch` appends cards. |
