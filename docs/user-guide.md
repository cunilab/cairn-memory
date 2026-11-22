---
id: CM-GUIDE-004
title: "Cairn Memory user guide"
status: draft
---

# Cairn Memory user guide

Set `CAIRN_SESSION_ORIGIN=automation` in the environment of a scheduled run, worker, or subagent. Cairn still captures that session and saves its distilled notes, and it does not ask to save at Stop and does not replace the handoff for that checkout. A person's own session in the same checkout is unchanged.
