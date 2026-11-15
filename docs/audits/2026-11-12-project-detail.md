---
id: CM-AUDIT-006
title: "Project detail display task 3 — 2026-11-12"
status: draft
---

# Project detail display task 3 — 2026-11-12

Read and display states on the project page. Share, revoke, delete, invite, and create-team handlers were not changed, and none of those buttons were clicked.

## Contract

`GET /projects/{id}/team` with no team returns HTTP 400 and `{"error":"validation error: project has no team space"}`. File-store `StoreError::Validation` and Postgres `PostgresError::Validation` both display that string. Other 400 messages, including `validation error: project already has a team space` and `validation error: account is not a project member`, stay errors.

## What changed

- `apps/web/lib/team.ts` treats only that exact no-team string as absence.
- `apps/web/components/project-detail.tsx` shows "No team yet. Create one with the form below." for an owner and "This project has no team." for a member. The create-team form stays for the owner. A different team failure stays a red alert and does not use the empty copy.
- A missing project shows "Project unavailable", the alert "Project is not on this account.", and no Decisions, Team, or Owner actions headings. The shell still links to Projects.
- The in-page Projects text link is gone. Desktop shows the sidebar Projects link. At 320 the header Toggle Sidebar button opens that same link.
- Memory text, ids, and metadata use `break-all` inside the card. Loading is `role="status"`.
- `scripts/smoke_web.sh` asserts the no-team classification and that nearby failures are not classified as absence.

## Checks

Isolated store `/tmp/cairn-task3-detail/store.json`, API `127.0.0.1:52213`, `next dev` `127.0.0.1:52214`. Owner and member accounts. Cairn Demo has no team and one shared memory whose content includes a 199-character unbroken token. Field Notes has team "Field team".

| Check | Result |
| --- | --- |
| Before, owner Cairn Demo at 1440 | Red alert `validation error: project has no team space`. Page overflow 288. The token paragraph overflowed by 599. |
| Before, unknown id | Alert present, and headings Decisions and Team still rendered. |
| After, owner, no team, 1440 / 390 / 320 | No red no-team alert. Empty copy and Create team present. Page overflow 0. Token paragraph overflow 0. |
| After, owner, Field Notes | Team name visible. No empty-team copy. No team alert. Create team absent because the team exists. |
| After, member, no team | "This project has no team." No Create team and no Owner actions. Overflow 0 at 1440 and 320. |
| After, unknown id | Title "Project unavailable". Alert "Project is not on this account." No section headings. Sidebar links "Cairn" and "Projects". |
| Keyboard | Tab reached Create team with `:focus-visible`. At 320, Toggle Sidebar opened a Projects link. |
| Intercepted team GET 400 `validation error: account is not a project member` | That string stayed in a red alert. The empty-team sentence was not shown. |
| Delayed team GET | "Loading project…" with `role="status"`, then the empty-team copy. The status was gone after load. |
| `node` classification check, `npm run typecheck`, `npm run lint` | Passed. `sh -n scripts/smoke_web.sh` passed. Production build was not run. |

## Screenshots

Before: `docs/audits/2026-11-12-project-detail/before/`.

After: `docs/audits/2026-11-12-project-detail/after/01-owner-noteam-1440.png`, `02-owner-noteam-390.png`, `03-owner-noteam-320.png`, `04-owner-team-1440.png`, `05-member-noteam-1440.png`, `06-missing-1440.png`, `07-create-focus-1440.png`, `08-team-error-1440.png`, `09-loading-1440.png`.

## Limits

DOM roles are not a screen-reader pass. A physical phone was not used. `next dev` draws its badge over the lower left of the 320 page. Delete, revoke, and the other owner buttons still run immediately. Confirmation for those actions was not part of this pass.
