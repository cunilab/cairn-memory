---
id: CM-REF-002
title: "Agent hooks"
status: draft
---

# Agent hooks

`cairn hook <Event>` reads the host event as JSON on stdin. It redacts captures before sending them to the server or storing them in the outbox. It reads the server and device credential from the selected client configuration. A missing configuration or `.cairnignore` disables capture for the checkout. Capture failures go to stderr. See [CLI and MCP](cli-and-mcp.md).

## Events

| Event | Action |
| --- | --- |
| `SessionStart` | Request a brief and instructions; check repository state when requested. |
| `UserPromptSubmit` | Capture the prompt. |
| `PreToolUse` (Claude Code) | Locally deny `Write`, `Edit`, `MultiEdit`, and `NotebookEdit` under the Claude config directory's `projects/*/memory/`. |
| `PostToolUse` | Capture edits and non-read-only shell commands; request topic context for edited files; recognize Cairn saves and lookups. |
| `Stop` | Checkpoint work and, when appropriate, ask once to save useful notes. |
| `PreCompact` | Checkpoint work. |
| `SessionEnd` | End a worked session for background distillation. |

Assistant messages are captured only with `CAIRN_CAPTURE_ASSISTANT=1`. Set `CAIRN_SESSION_ORIGIN=automation` for automated sessions; they capture and distill but do not receive the Stop ask or replace the person's handoff. The hook detects subagent sessions similarly.

## Reminders (agent nudges)

The default start instruction tells agents to use Cairn as their only memory, call `memory_search` before deciding, debugging or acting on unfamiliar context, and save decisions, fixes and corrections with a reason. Setup includes the same instruction in the agent's global instructions. The Stop reminder can ask the host to save up to three useful notes after work. It runs at most once per eligible session. A Cairn save counts only when the CLI exits successfully or the MCP result saved or deduplicated at least one item; a failed save still allows the ask. Each command execution is kept, so the digest reports the last result of a repeated test. OpenCode does not support continuing the turn from the Stop reminder. For the hook, project instructions override global instructions, which override built-in text. Edit these overrides in the web console. Setting the Stop text to `off` disables the ask for that scope; an empty text falls back to the next level. Set `nudge` to `false` in the client configuration or `CAIRN_NO_NUDGE=1` to disable nudges.

## Injection budget

The server brief is limited to 1,200 bytes. Up to four topic injections of at most 300 bytes each are allowed, with at most 16 topic lookups per session. The session instruction and the Stop text are each limited to 600 bytes, globally and per project.

## Redaction

The hook and server use the same ordered patterns, replacing secrets with `[REDACTED]` before storage. This is a best-effort filter, not a guarantee that every secret is recognized. The following table describes the patterns in `crates/core/src/capture/redact.rs`.

| # | Pattern | Replacement |
|---|---|---|
| 1 | PEM private-key blocks (`redact_private_key_blocks`) | whole block |
| 2 | `(?i)\b(authorization\s*:\s*(?:bearer|basic|token|digest))\s+[^\s"']+` | `$1 [REDACTED]` |
| 3 | `(?i)\b([a-z][a-z0-9+.-]*://)[^\s/:@]+(?::[^\s/@]+)?@` (user alone or user:password) | `$1[REDACTED]@` |
| 4 | `(?i)(--(?:password|passwd|pass|token|secret|api-key|apikey|access-key|client-secret))(=|\s+)(\[REDACTED\]|"[^"]*"|'[^']*'|\S+)` | `$1$2[REDACTED]` |
| 5 | `(?i)([a-z0-9_.-]*(?:api[_-]?key|secret|token|password|passwd|pwd|access[_-]?key)[a-z0-9_]*)(["']?\s*[=:]\s*)(\[REDACTED\]|"[^"]*"|'[^']*'|[^\s,;&]+)` (a prefixed name such as `DB_PASSWORD`, a closing quote as in JSON) | `$1$2[REDACTED]` |
| 6 | `(?i)(\bmysql\w*\b[^\n]*?\s-p)\S+` | `$1[REDACTED]` |
| 7 | `(?i)(\bcurl\b[^\n]*?\s(?:-u\|--user)\s+)[^\s:]+:\S+` | `$1[REDACTED]` |
| 8 | `\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36,}\b` and `\bgithub_pat_[A-Za-z0-9_]{22,}\b` | `[REDACTED]` |
| 9 | `\b(?:AKIA|ASIA)[A-Z0-9]{16}\b` | `[REDACTED]` |
| 10 | `\bxox[abprs]-[A-Za-z0-9-]{10,}\b` | `[REDACTED]` |
| 11 | `\bsk-(?:ant-|proj-)?[A-Za-z0-9_-]{20,}\b` | `[REDACTED]` |
| 12 | `\bsk_(?:live|test)_[A-Za-z0-9]{10,}\b` | `[REDACTED]` |
| 13 | `\bnpm_[A-Za-z0-9]{36,}\b` | `[REDACTED]` |
| 14 | `\bglpat-[A-Za-z0-9_-]{20,}\b` | `[REDACTED]` |
| 15 | `\bAIza[A-Za-z0-9_-]{35}\b` | `[REDACTED]` |
| 16 | `\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b` (JWT) | `[REDACTED]` |
| 17 | `\bcdt_[A-Za-z0-9_-]{16,}\b` (Cairn device token) | `[REDACTED]` |

## Outbox

The bounded outbox in the configuration directory (or `CAIRN_OUTBOX`) is bound to the server origin and a fingerprint of the device credential. Retryable delivery failures are queued; successful requests replay queued events. Authentication and permanent client errors are not queued. A queue bound to another origin or credential is not reused. The queue holds up to 256 events; when full, the oldest event is dropped. Each event has at most 10 attempts. A busy lock or write error can prevent retention; inspect hook stderr and run `cairn doctor` when capture is unhealthy.

## Setup

Run `cairn setup <url> --agent claude|codex|opencode` and supply the device token on stdin. Setup writes managed hook and MCP entries and adds a marked instruction block. Upgrades refresh installed integrations. Passing `--remove` removes the block and managed entries while keeping user text. Codex and OpenCode setup need their native CLI on `PATH`. Codex also requires trusting the hooks. `cairn doctor` checks the configuration.

| Agent | Global instruction file |
| --- | --- |
| Claude Code | `${CLAUDE_CONFIG_DIR:-~/.claude}/CLAUDE.md` |
| Codex | `${CODEX_HOME:-~/.codex}/AGENTS.md` |
| OpenCode | `${XDG_CONFIG_HOME:-~/.config}/opencode/AGENTS.md` |

Codex reads a nonempty global `AGENTS.override.md` before `AGENTS.md`. [Codex instruction discovery](https://developers.openai.com/codex/guides/agents-md)

Claude Code setup installs a `PreToolUse` guard matching the four file-writing tools. The guard returns the documented `deny` decision for native memory paths and exits silently for other paths. It uses the filesystem without contacting Cairn. Use `cairn setup <url> --agent claude --keep-native-memory` to omit the guard across updates. This affects the guard only. [Claude Code hook decisions](https://code.claude.com/docs/en/hooks#pretooluse-decision-control)

Claude Code setup and update refresh add `mcp__cairn` to `permissions.allow` in `settings.json` when neither server-wide rule exists. Removal deletes the bare rule, including a user-added one, and keeps the wildcard rule and permission containers. Codex setup and refresh set `default_tools_approval_mode = "approve"` on `mcp_servers.cairn` in `config.toml`, which also enables Cairn MCP calls in `codex exec`. [Codex configuration reference](https://developers.openai.com/codex/config-reference)
