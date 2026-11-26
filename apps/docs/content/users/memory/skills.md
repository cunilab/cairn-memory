---
id: CM-DOC-USERS-MEMORY-SKILLS
title: "Skills"
status: accepted
description: "Find reusable instructions, publish a numbered revision, or roll back a skill."
---

# Skills

The shared library is global. Search with `skill_search` and read a version with `skill_get`. Supplying no version reads the current one. A prompt can bring in one matching skill per session within the existing automatic recall budget. Hermes gateway chat prompts omit these automatic skill cards and can still call the skill tools.

Publish with `skill_save` and a stable lowercase name, title, description, Markdown body and change note. Reusing the name appends a revision. A vote with `skill_feedback` applies to one version and replaces your previous vote on it.

In the console, **Skills** shows history and can roll back by publishing a copy of an earlier revision. Administrators can archive and restore a skill. Archived skills are hidden from agent search. Eligible, visible procedures can appear as conversion suggestions. Review and publish the skill yourself, or dismiss the suggestion. [MCP contracts](../reference/mcp.md#tools).
