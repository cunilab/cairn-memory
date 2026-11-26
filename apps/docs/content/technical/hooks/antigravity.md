---
id: CM-DOC-TECHNICAL-HOOKS-ANTIGRAVITY
title: "Antigravity CLI hooks"
status: accepted
description: "PreInvocation input, deferred context and the available lifecycle events."
---

# Antigravity CLI hooks

SessionStart can inject the brief. PreInvocation extracts the latest user input, deduplicates repeated invocations of the same prompt within an execution, and supplies prompt recall through `injectSteps`. PostToolUse captures tool activity and can queue failed-command cards for a one-time system message in the next PreInvocation.

For `run_command`, PreToolUse can return `allow` with a reason for an exact command warning. The host's permissions still apply. Delivery of that reason to the model is unverified. PostToolUse supplies neither file topic injection nor MCP result contents. Stop checkpoints work. The hook API has no compact or session-end event.

[Host delivery matrix](index.md#command-recall-by-host) · [Antigravity CLI setup](../../users/agents/antigravity.md).
