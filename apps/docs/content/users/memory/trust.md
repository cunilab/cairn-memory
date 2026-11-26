---
id: CM-DOC-USERS-MEMORY-TRUST
title: "Trust and corrections"
status: accepted
description: "Read Cairn’s trust labels, correct outdated knowledge and forget your own records."
---

# Trust and corrections

## Trust labels

Cairn applies these labels in order:

1. **Doubted** is stale or reported wrong.
2. **New** is a candidate awaiting confirmation.
3. **Proven** has at least two helpful votes or three saves of the same memory.
4. **Normal** covers the remaining memories.

Candidates rank lower. Helpful feedback can confirm a coached agent save. Two distinct wrong votes, or a single wrong vote from the owner, mark the record stale. Repeating the same vote as the same person does not add a second vote.

## Corrections, links and forgetting

Supply `supersedes` when replacing an owned memory or a same-project record your account can correct. Save a declared `contradicts` link when you cannot replace the other record. The replaced or stale record remains searchable with a label. To hide a record you own, use `memory_forget` or `cairn forget ID`. Age alone does not hide records.

## Read the counters

Open a memory's sheet to see why the label applies. The **Shown** counter tracks hook delivery. **Used** includes explicit searches and gets that returned the record. The sheet also reports requests for full detail. [Recall layers](layers.md) explains automatic eligibility and muting.
