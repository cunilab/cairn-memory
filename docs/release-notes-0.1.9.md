---
id: CM-REL-005
title: "Cairn Memory 0.1.9 release notes"
status: draft
---

# Cairn Memory 0.1.9 release notes

This release improves project capture, handoffs, retrieval and the web console. It adds Fixes, keeps memories searchable as they age, and retains the five existing MCP tools. Token estimates below are approximate and apply only when the described context is shown to the agent.

## Changes and token costs

| Item | Change | Agent token cost |
| --- | --- | --- |
| 01 | Distilled notes attach to a recognized project and start as lower-ranked candidates; use or confirmation promotes them. | No added context; candidates are excluded from briefs. |
| 02 | Projects are recognized with or without Git. `cairn init` can initialize a non-Git folder; an unrecognized project gets a hint. | About 22 tokens for the hint, once per session without a project. |
| 03 | After work, the Stop hook asks once for up to three reusable notes, saved in one `memory_save` call. | About 40 tokens for the ask; up to about 110 tokens added to the tool schema. Agent-written notes cost output tokens. |
| 04 | Sessions with changed files produce a searchable project digest, including changed files and test results. | No brief tokens; a search hit costs a normal card, about 60–100 tokens. |
| 05 | A short chat no longer overwrites a useful handoff; explicit next steps and changed files are preserved. | No added agent tokens. |
| 06 | A detected user correction adds a request to save a reusable rule to the existing Stop ask. | About 25 tokens plus the quote, only when a correction is detected. |
| 07 | A write-side benchmark checks capture, handoffs, digests and the save ask in CI. | None at runtime. |
| 08 | The project page reports recent sessions, their changes, saves and reasons for missing output. | None; web only. |
| 09 | Saves can include up to five aliases, and existing memories receive derived aliases. Alias vector search remains disabled because its benchmark introduced weak-query hits. | About 45 schema tokens per session; agent-written aliases add about 20–60 output tokens per note. Search response size is unchanged. |
| 10 | A local cross-encoder was evaluated but is not shipped: it increased weak-query hits and latency. | None. |
| 11 | Repeated knowledge records a proof count and receives a small ranking boost; material differences such as numbers or file names prevent automatic merging. | About three tokens for `seen`, only when the count is at least two. |
| 12 | Automated and subagent sessions still capture and distill, but do not receive the Stop ask or overwrite a person's handoff. Use `CAIRN_SESSION_ORIGIN=automation` for automated sessions. | Fewer tokens in those sessions because there is no ask. |
| 13 | Cairn records data for Fixes, including missed searches, hook errors and whether the save ask was followed. | None in agent context. |
| 14 | Server detectors identify issues and create Fixes with evidence and an action. | None in agent context. |
| 15 | Admins can review Fixes in the web console, apply supported actions, ignore issues or copy an agent prompt. | None during normal sessions; a copied prompt costs about 60 tokens if pasted. |
| 16 | Applied computer fixes run on their named device through `cairn fixes`. Opening a known project using its `.cairn/project.json` joins the person and records an audit event. | None in agent context. |
| 17 | Age no longer hides memories. Stale and replaced memories remain searchable with labels and lower ranking; forgotten memories remain hidden. | No added bytes on active cards; up to 16 bytes on stale and 40 on replaced cards. |
| 18 | The session-start instruction tells agents to use Cairn as their memory. Built-in agent memory remains enabled; nothing is copied or blocked. | About 130 additional bytes per session start and 70 bytes once in MCP instructions; outside a project the start line is about 260 bytes. |

Agent instructions and the Stop reminder are editable in the web. Set `nudge` to `false` in the client config or set `CAIRN_NO_NUDGE=1` to disable nudges. Search ranking now includes proof and status; the release does change retrieval. See [the benchmark analysis](../bench/README.md) for the gate and benchmark limits.

## Database migrations

The server applies these migrations on startup:

| Migration | Purpose |
| --- | --- |
| `0006_agent_instructions` | Project instruction overrides. |
| `0007_memory_rank` | Candidate and normal rank. |
| `0008_proof_count` | Proof counts and backfill. |
| `0009_memory_aliases` | Alias table, index and derived alias backfill. |
| `0010_session_outcome` | Session distillation outcomes. |
| `0011_fixes` | Fixes, lookup logs and hook status. |
| `0012_always_active` | Search index including superseded memories. |
| `0013_link_origin_system` | System link origin and handoff link backfill. |
| `0014_memory_owner_project_created` | Project reporting index. |

Migration `0013` replaces a constraint to allow the new value and retains existing link rows. See the migration guard allowlist for the approved constraint change.

## Upgrade

1. Back up the database and upgrade the server and clients together. The server applies the migrations at startup.
2. On each machine run `cairn update`, then rerun `cairn setup --agent <agent>` for each agent so hooks recognize MCP saves.
3. Run `cairn doctor` and address any failures.

An unknown project ID now requires explicit registration with `cairn init --yes`; an older client receives a 409 rather than silently registering it. Existing known projects can still be joined.
