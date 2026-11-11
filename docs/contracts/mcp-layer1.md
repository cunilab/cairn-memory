---
id: CM-CONTRACT-002
title: "Layer 1 MCP Compatibility Contract"
status: draft
---

# Layer 1 MCP Compatibility Contract

This contract defines the first Cairn MCP transport over the existing
`cairn-core` service boundary. It is intentionally a small, local compatibility
surface rather than a claim of parity with the upstream full tool registry.

## Transport

- Command: `cairn-server mcp --store <path>`
- Transport: newline-delimited JSON-RPC 2.0 over stdin/stdout
- Diagnostics: stderr only; stdout is reserved for protocol messages
- Handshake: `initialize`, `notifications/initialized`
- Discovery: `tools/list`
- Invocation: `tools/call`
- Probe: `ping`
- Protocol version returned by this adapter: `2025-06-18`

## Tool manifest

The adapter exposes the upstream Layer 1 core names and the direct capture
aliases needed to use Cairn without a separate REST client:

| Tool | Layer 1 behavior |
| --- | --- |
| `memory_save` | Save explicit durable memory |
| `memory_recall` | Return a bounded context pack |
| `memory_consolidate` | End a session and create its evidence-backed memory |
| `memory_smart_search` | Search durable memories |
| `memory_why_changed` | Return decisions, reasons, evidence, relationships, and conflict signals |
| `memory_sessions` | List local sessions |
| `memory_diagnose` | Report local storage and capture counts |
| `memory_lesson_save` | Save an explicit memory classified as a lesson |
| `memory_reflect` | Return retrieval candidates; no model is invoked |
| `memory_session_start` | Start or resume a session |
| `memory_observe` | Capture one bounded, idempotent observation |
| `memory_session_end` | Alias for session consolidation |
| `memory_search` | Keyword search |
| `memory_context` | Bounded context assembly |
| `memory_resume` | Recover recent session summaries for current work context |
| `memory_unresolved` | Recover session summaries with unresolved work |
| `memory_forget` | Persist forget suppression for an event or memory |
| `memory_export` | Return a versioned portable memory bundle |
| `memory_import` | Merge a versioned bundle idempotently |
| `memory_correct` | Supersede a memory or observation with a corrected durable memory |

Tool results include a text content block and `structuredContent`. Failed
memory operations are returned as tool results with `isError: true`; malformed
JSON-RPC requests and unknown protocol methods use JSON-RPC errors.

Capture and explicit-save tools accept the nested Layer 1 `lifecycle` object
defined in [CM-CONTRACT-004](lifecycle-v1.md). `memory_diagnose` includes the
current lifecycle counts.

## Layer 2 context filters

The query tools `memory_recall`, `memory_context`, `memory_reflect`,
`memory_smart_search`, and `memory_search` accept optional `sessionId` and
`context` arguments. Their scope behavior is defined in
[CM-CONTRACT-006](continuity-v1.md).

## Deliberate limitations

- The adapter is local and file-backed; authentication, authorization, and
  cross-device synchronization are not implemented. Layer 2 scope filtering is
  local context matching, not an authorization check.
- `memory_reflect` is retrieval-only until a provider-backed reflection
  lifecycle exists.
- The adapter supports the compatibility handshake used by the current Layer 1
  baseline; modern protocol-era discovery is a later transport task.
- The tool schemas are Cairn-owned compatibility schemas. Exact upstream
  argument and result parity remains a verification task.

## Evidence

- `crates/server/src/mcp.rs` contains protocol handling and unit tests.
- `scripts/smoke_mcp.sh` drives the standalone process through initialize,
  discovery, capture, consolidation, recall, and forget.
