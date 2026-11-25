---
id: CM-GUIDE-004
title: "Cairn Memory user guide"
status: draft
---

# Cairn Memory user guide

Cairn helps agents resume work after a break or compaction, hand off between machines, share project decisions and avoid repeating gotchas. People inspect and correct memory through the web console.

## Concepts

Memories can be decisions, rules, gotchas, procedures, facts, preferences, notes or system handoffs. Personal memories are private to their owner; project memories are visible to project members; global memories are visible to server users. Automatic notes start as lower-ranked candidates and can become normal through use or confirmation. Links describe relationships such as `supersedes` and `contradicts`. Stale and replaced records remain searchable with a label; memories do not expire by age.

## Set up

[Deploy the server and console](deploy.md), then create the first account in the web. It is the only administrator and can create additional accounts. Create a device token in the web, then run:

```sh
cairn setup https://your-server.example --agent claude
cairn doctor
```

Setup reads the token from stdin. Choose `claude`, `codex`, `opencode`, `grok` or `antigravity`. Codex and OpenCode setup need their native CLI installed. For an already configured device, run `cairn setup <agent>`. See [hook setup](reference/hook.md#setup).

## Projects

Run `cairn init "Project name"` in a project folder and commit `.cairn/project.json`. The file connects checkouts even without Git. To rename a project, the owner uses its page in the web console. Teammates see a session-start notice; run `cairn init` to update the project file and commit it. An unknown ID requires `cairn init --yes` to register it. **Anyone with an account on the same server and the project ID can join and read project memory.** See [projects](reference/projects.md#init-and-membership).

## Day-to-day use

Hooks capture prompts, edits and non-read-only commands. In a recognized project, the Stop hook can ask once to save useful notes after edits or a correction (not on OpenCode). Use `cairn search`, `cairn get`, `cairn save`, `cairn feedback` and `cairn forget`, or their five MCP equivalents. To correct a memory, save the replacement with `supersedes` set to its ID. Add `.cairnignore` to disable capture in a folder. Assistant messages are captured only if `CAIRN_CAPTURE_ASSISTANT=1`.

The hook provides a session-start instruction and a Stop reminder. The admin edits the global texts in Settings; a project owner or admin edits a project's texts on its page. Set `nudge` to `false` in the client configuration or `CAIRN_NO_NUDGE=1` to disable them. Set `CAIRN_SESSION_ORIGIN=automation` for scheduled or automated work: capture and distillation continue, while the Stop ask and personal handoff replacement are suppressed.

## Web console and Fixes

Project owners and the administrator can archive a project from its page and restore it from **Projects → Archived**. Members can read the archived project’s memories there, or select the project on the **Memories** page. Archiving hides those memories from agents until restore.

Dashboard summarizes health and activity and does not count memories as expiring; Memories lists records; Activity shows sessions, jobs and audit; Projects shows members and session reports; Graph shows links; Settings manages account, devices and instructions. A session report's save count is new records during that session's time window, so one row can appear in two overlapping sessions. Fixes gives the admin evidence and actions for detected issues. The admin can apply or ignore a fix, or copy a prompt for the agent. A computer fix is queued for its device. It runs the next time someone there runs `cairn doctor`, `cairn init` or `cairn fixes apply [id]`; `cairn fixes` lists it, and an update is finished only when the installed version reaches the target. Applying a fix can also turn off a project’s Stop ask, lower a long-unused memory’s rank, or undo a wrong automatic merge. Secret redaction covers the title, summary, reason, and kind fields of the live memory. A copy can remain in evidence or an older version; rotate the secret.

## Privacy and data

Capture and saves are redacted before storage using best-effort patterns; check sensitive data before sharing it. Raw observations are retained for processing, not returned by memory search, and expire after 30 days; captures from a completed, distilled session can be removed after seven days. Forget hides a retained record from normal retrieval. Export is available in the console. Administrators can inspect redacted recent search phrases and rewrite memories; rewrites preserve the original and show that an admin changed it. Scope and membership determine who can see project or global memory.

## Updates and troubleshooting

Upgrade server and clients together and read the release notes. Run `cairn update`, refresh each integration with `cairn setup`, and run `cairn doctor`. Check hook stderr and the outbox when delivery fails, server health for `semantic_search`, and Activity for quarantined jobs. See [recall](reference/recall.md) and [hook details](reference/hook.md).
