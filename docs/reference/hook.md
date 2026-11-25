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
| `UserPromptSubmit` | Capture the prompt and check for relevant preferences or project memories. |
| `PreToolUse` | Look up an exact command warning. Claude Code also checks native memory writes locally. |
| `PostToolUse` | Capture edits and non-read-only shell commands, request file topics, recognize Cairn saves and lookups, and check failed commands for related memories. |
| `PostToolUseFailure` (Claude Code) | Recall related memories when the shell tool fails. |
| `Stop` | Checkpoint work and, when appropriate, ask once to save useful notes. |
| `PreCompact` | Checkpoint work. |
| `SessionEnd` | End a worked session for background distillation. |

Assistant messages are captured only with `CAIRN_CAPTURE_ASSISTANT=1`. Set `CAIRN_SESSION_ORIGIN=automation` for automated sessions; they capture and distill but do not receive the Stop ask or replace the person's handoff. The hook detects subagent sessions similarly.

## Reminders (agent nudges)

The default start instruction tells agents to use Cairn as their only memory, call `memory_search` before deciding, debugging or acting on unfamiliar context, and save decisions, fixes and corrections with a reason. Setup includes the same instruction in the agent's global instructions. The Stop reminder can ask the host to save up to three useful notes after work. It runs at most once per eligible session. A Cairn save counts only when the CLI exits successfully or the MCP result saved or deduplicated at least one item; a failed save still allows the ask. Each command execution is kept, so the digest reports the last result of a repeated test. OpenCode does not support continuing the turn from the Stop reminder. For the hook, project instructions override global instructions, which override built-in text. Edit these overrides in the web console. Setting the Stop text to `off` disables the ask for that scope; an empty text falls back to the next level. Set `nudge` to `false` in the client configuration or `CAIRN_NO_NUDGE=1` to disable nudges.

## Injection budget

The server allows eight pushed memories per person and session ID across briefs, topics, command recall and prompt recall. The brief fits within 1,800 bytes. Hooks request up to 16 file topics and can inject up to four topic replies of 300 bytes each. The session instruction and the Stop text are each limited to 600 bytes, globally and per project. See [automatic recall](recall.md#automatic-recall).

The stdio MCP server also requests the brief for its working directory and appends it to the initialize instructions. The HTTP request has a one-second timeout. An unavailable or empty brief leaves the static instructions in place. A host can supply its hook session ID via `initialize.params._meta.sessionId` to share the eight-memory budget. Standard stdio initialization carries no host session ID, so a client that omits this field can receive the same memory through MCP and a hook. [MCP initialization](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle)

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

Run `cairn setup <url> --agent claude|codex|opencode|grok|antigravity|hermes` and supply the device token on stdin. On an already configured device, run `cairn setup <agent>`. Setup writes managed hook and MCP entries and adds a marked instruction block. Upgrades refresh installed integrations. Run `cairn setup <agent> --remove` to remove the integration while keeping the device configuration. Codex and OpenCode setup need their native CLI on `PATH`. Codex also requires trusting the hooks. `cairn doctor` checks the configuration.

| Agent | Global instruction file |
| --- | --- |
| Claude Code | `${CLAUDE_CONFIG_DIR:-~/.claude}/CLAUDE.md` |
| Codex | `${CODEX_HOME:-~/.codex}/AGENTS.md` |
| OpenCode | `${XDG_CONFIG_HOME:-~/.config}/opencode/AGENTS.md` |
| Grok Build | `${GROK_HOME:-~/.grok}/AGENTS.md` |
| Hermes Agent | `$HERMES_HOME/SOUL.md` (typically `~/.hermes/SOUL.md`) |
| Antigravity CLI | `~/.gemini/config/AGENTS.md` |

Codex reads a nonempty global `AGENTS.override.md` before `AGENTS.md`. [Codex instruction discovery](https://developers.openai.com/codex/guides/agents-md)

Claude Code setup matches Bash and the four file-writing tools `Write`, `Edit`, `MultiEdit` and `NotebookEdit`. The local write guard denies those tools under the Claude config directory's `projects/*/memory/`. Use `cairn setup <url> --agent claude --keep-native-memory` to keep native writes enabled across updates while keeping the Bash warning hook. [Claude Code hook decisions](https://code.claude.com/docs/en/hooks#pretooluse-decision-control)

Claude Code setup and update refresh add `mcp__cairn` to `permissions.allow` in `settings.json` when neither server-wide rule exists. Removal deletes the bare rule, including a user-added one, and keeps the wildcard rule and permission containers. Codex setup and refresh set `default_tools_approval_mode = "approve"` on `mcp_servers.cairn` in `config.toml`, which also enables Cairn MCP calls in `codex exec`. [Codex configuration reference](https://developers.openai.com/codex/config-reference)

Grok Build registers `mcp_servers.cairn` and grants `MCPTool(cairn__*)` in `config.toml`. The Cairn hooks live in `hooks/cairn.json` under Grok's config directory. SessionStart records the checkout baseline, and the first PostToolUse delivers the brief because Grok ignores SessionStart output. Grok also runs prompt capture, Stop, PreCompact and SessionEnd. When both integrations are installed, Cairn captures Grok activity through its native hooks once. Use `grok inspect` to check discovery. [Grok MCP](https://docs.x.ai/build/features/mcp-servers), [Grok permissions](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/22-permissions-and-safety.md), [Grok hooks](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/10-hooks.md), [Grok rules](https://docs.x.ai/build/features/project-rules)

Antigravity CLI registers Cairn in `~/.gemini/config/mcp_config.json` and grants `mcp(cairn/*)` in `~/.gemini/antigravity-cli/settings.json`. Explicit ask and deny rules still apply. Its named Cairn group in `~/.gemini/config/hooks.json` injects the brief on SessionStart, captures the last user input through PreInvocation, captures tool activity on PostToolUse and checkpoints on Stop. The hook API has no compact or session-end event, and PostToolUse supplies neither topic injection nor MCP result contents. Use `agy mcp list` to check the registration. [Antigravity discovery](https://antigravity.google/docs/customization-discovery/), [Antigravity permissions](https://antigravity.google/docs/agent-permissions), [Antigravity hooks](https://antigravity.google/docs/hooks)

Hermes Agent registers `mcp_servers.cairn` in the profile's `config.yaml` with `trust: full` scoped to Cairn. Setup enables `plugins/cairn-memory/` through `plugins.enabled` and adds the marked block to `SOUL.md`. Run setup with `HERMES_HOME` pointing at your profile to target a profile other than the default. The YAML rewrite preserves unrelated settings and may change comments and formatting. Use `hermes mcp list` and `hermes plugins list` to check discovery after restarting Hermes. [Hermes profile paths](https://github.com/NousResearch/hermes-agent/blob/f85f33bf534d0a4ac88c348d34a4c9ddb424dcb7/hermes_constants.py), [Hermes MCP](https://hermes-agent.nousresearch.com/docs/user-guide/features/mcp), [Hermes MCP trust](https://github.com/NousResearch/hermes-agent/blob/f85f33bf534d0a4ac88c348d34a4c9ddb424dcb7/tools/mcp_tool_registration.py), [Hermes plugins](https://hermes-agent.nousresearch.com/docs/developer-guide/plugins), [Hermes context files](https://hermes-agent.nousresearch.com/docs/user-guide/features/context-files)

The plugin uses `pre_llm_call` to deliver the brief with the first user prompt and deliver prompt recall before model calls. It forwards after tool calls, checkpoints at turn completion and after committed compaction, and closes on session teardown. Capture requires a working directory supplied by Hermes. Per-file notes and continuing the model loop with a Stop reminder are unavailable on Hermes. [Hermes hooks](https://hermes-agent.nousresearch.com/docs/user-guide/features/hooks), [Hermes plugin session context](https://hermes-agent.nousresearch.com/docs/user-guide/features/plugins)


## Command recall by host

Setup refresh adds the new managed hooks to existing installs. A pre-tool lookup may run before the tool even when the host delivers its context with the result. Recall uses a one-second client timeout.

| Host | Warning lookup and delivery | Failed-command recall |
| --- | --- | --- |
| Claude Code | Bash `PreToolUse` returns `hookSpecificOutput.additionalContext` without a permission override. Claude delivers it alongside the tool result. | `PostToolUseFailure` adds context alongside the error. [Claude Code hooks](https://code.claude.com/docs/en/hooks#pretooluse-decision-control) |
| Codex | Bash `PreToolUse` returns `hookSpecificOutput.additionalContext`. | `PostToolUse` can add context when its payload reports a failure. Unified exec can supply plain output without an exit status; Cairn stays silent in that case. [Codex hooks](https://developers.openai.com/codex/hooks), [Codex unified-exec source](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/context.rs) |
| Grok Build | Bash `PreToolUse` looks up the warning before execution. Grok delivers `additionalContext` with the tool batch result. | Bash `PostToolUse` includes nonzero exits and accepts `additionalContext`. [Grok hooks](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/10-hooks.md#posttooluse-output) |
| Antigravity CLI | `run_command` `PreToolUse` returns `allow` with a reason and preserves default permissions. Model delivery of an allow reason is unverified. | `PostToolUse` queues matched cards for a one-time `PreInvocation.injectSteps` system message before the next model call. [Antigravity hooks](https://antigravity.google/docs/hooks#input-and-output-contract) |
| OpenCode V2 | `ctx.tool.hook("execute.before")` runs before the shell command. Its return is void, so Cairn passes the warning through the next `ctx.session.hook("context")`. | `execute.after` supplies completed and failed results. The next model context receives matched cards. [OpenCode V2 server plugins](https://opencode.ai/docs/build/plugins/server) |
| Hermes Agent | The `pre_tool_call` policy event has no advisory context return. Cairn looks up warnings in `post_tool_call`. | `post_tool_call` queues cards for `transform_tool_result` to attach to the terminal result. With no result transform, `pre_llm_call` can deliver them on the next user turn. [Hermes hooks](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/features/hooks.md#transform_tool_result) |


## Prompt recall by host

| Host | Delivery |
| --- | --- |
| Claude Code and Codex | `UserPromptSubmit` returns `hookSpecificOutput.additionalContext`. [Claude Code hooks](https://code.claude.com/docs/en/hooks#userpromptsubmit), [Codex hooks](https://developers.openai.com/codex/hooks#userpromptsubmit) |
| Grok Build | `UserPromptSubmit` runs the lookup. Grok discards advisory output on that event, so Cairn delivers the cards once with the next `PostToolUse` result. [Grok hooks](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/10-hooks.md#userpromptsubmit-decision-control) |
| Antigravity CLI | `PreInvocation` supplies the latest user text and injects cards through `injectSteps`. Cairn deduplicates repeated model invocations for the same prompt within one execution. [Antigravity hooks](https://antigravity.google/docs/hooks#preinvocation) |
| OpenCode V2 | The prompt hook queues matched cards for the next context hook. [OpenCode V2 server plugins](https://opencode.ai/docs/build/plugins/server) |
| Hermes Agent | `pre_llm_call` includes the cards in the context for the model call. [Hermes hooks](https://hermes-agent.nousresearch.com/docs/user-guide/features/hooks) |

Refresh managed setup to pick up the OpenCode and Hermes plugin changes and the feedback tool matcher.
