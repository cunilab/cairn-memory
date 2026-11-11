---
id: CM-STD-001
title: "Cairn Memory Documentation Standards"
status: draft
---

# Cairn Memory Documentation Standards

These standards apply to everything under `docs/`. The root `README.md` is a short entry point and has no YAML header.

The goal is documentation that helps people understand, decide, build, review, release, or operate Cairn Memory without creating a second product to maintain.

## Header

Every governed document starts with this YAML header and nothing before it:

```yaml
---
id: CM-<AREA>-NNN
title: "<Matches the H1 heading>"
status: draft
---
```

| Field | Rule |
| --- | --- |
| `id` | Permanent and unique. Never reuse an ID, even if a file moves. |
| `title` | Matches the H1 heading and stays quoted. |
| `status` | One of `draft`, `proposed`, `accepted`, or `superseded`. |

Status describes the document, not the software. A `draft` document may describe a planned design; it must not imply that the design is implemented.

## One source of truth per topic

| Topic | Owner |
| --- | --- |
| Product purpose, users, collaboration model, goals, and boundaries | [`product.md`](./product.md) |
| Product and rewrite phases and milestone outcomes | [`roadmap.md`](./roadmap.md) |
| System structure, data ownership, and flows | [`architecture.md`](./architecture.md) |
| Rationale for expensive or hard-to-reverse choices | [`adr/`](./adr/) |
| Feature behavior and acceptance criteria | `specs/` documents |
| Upstream public contracts and compatibility classification | `contracts/` documents |
| Pass/fail checks and recorded results | `evidence.md` |
| Automatic lifecycle, privacy, ownership, and sharing policy | `policy.md` |
| User installation and usage | Future `user-guide.md` |
| Runtime operations and recovery | Future `operations.md` |

If two documents contain the same rule, keep the rule in its owner and link to it elsewhere.

## Changing documents

- Lifecycle: `draft` → `proposed` → `accepted` → `superseded`.
- A substantive edit to an `accepted` document returns it to `proposed` until reviewed again.
- A public contract, storage format, dependency direction, protocol, or migration strategy change needs an ADR or an ADR update in the same change.
- A change to source-of-truth ownership, authentication, branch/release workflow, or retrieval model needs an ADR or an ADR update in the same change.
- New documents must use a new ID and add a row to [`docs/README.md`](./README.md).
- Roadmap entries describe outcomes and gates, not every implementation task.
- PRDs describe feature behavior and boundaries, not implementation diaries.
- ADRs record why a choice was made, not meeting notes.
- Evidence documents record what was proven and how; plans do not belong there.

## Evidence discipline

Use careful language:

- `planned` means the roadmap proposes it.
- `designed` means a document describes it.
- `implemented` requires a code reference and a check.
- `verified` requires a recorded command, test, or user-level observation.

Do not put benchmark numbers, compatibility claims, or “works” statements into product, roadmap, or architecture documents unless they link to recorded evidence.

## Review checklist

- [ ] Header has exactly `id`, `title`, and `status`.
- [ ] ID is unique and permanent.
- [ ] H1 matches the header title.
- [ ] Relative links resolve.
- [ ] The document owns one topic and does not duplicate another document.
- [ ] Planned work is not described as implemented.
- [ ] Public contract or migration changes have an ADR.
- [ ] The document is linked from [`docs/README.md`](./README.md).
