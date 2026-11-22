---
id: CM-AUDIT-005
title: "Projects list task 2 — 2026-11-12"
status: draft
---

# Projects list task 2 — 2026-11-12

The projects list is the only screen changed in this pass. Project detail, authentication, and packages were left as they were.

## Sources

- [Empty](https://ui.shadcn.com/r/styles/base-nova/empty.json), fetched 2026-11-12. The base-nova empty block is a centered dashed region with a short title and a muted description, and no required action. `empty.tsx` was not added. The zero-project state uses those classes on the list page.
- Existing `Card`, `CardHeader`, `CardTitle`, `CardDescription`, and `CardContent` in `apps/web/components/ui/card.tsx`. The list item is one of those cards.

## What changed

`apps/web/app/projects/page.tsx` only.

- The whole card is one link. The name is not a separate link, and the card contains no button or field.
- Owner and Member are labeled from the API `role`. Any other role string is shown unchanged.
- Long names and repositories use `break-words`. The project id uses `break-all`.
- Loading is `role="status"`. A list failure keeps the server text after "Projects could not be loaded." A logout failure is a separate sentence. A missing or revoked session still replaces the page with `/`.
- Zero projects says "No projects" and "No projects for this account." There is no create control.

## Checks

Isolated file store `/tmp/cairn-task2-list/store.json`, API `127.0.0.1:51826`, `next dev` `127.0.0.1:51827`. Accounts `owner@example.test`, `member@example.test`, and `empty@example.test`. The owner has Cairn Demo and a 135-character name with an 84-character repository. The member is invited only to Cairn Demo.

| Check | Result |
| --- | --- |
| Before, owner list | The name link measured 79×19. Screenshots in `before/`. |
| After, 1440 | The Cairn Demo card link measured 1096×114. No nested link, button, or field. Page overflow 0. |
| Keyboard | Tab reached that link with `:focus-visible` and a 3px ring. Enter opened `/projects/project-398d8b0e6a9ebc09c39921f4`. |
| 390 and 320 | Page overflow 0. Card heights 114, and 200 or 220 for the long name. Title, repository, and id overflow 0. |
| Member | One card, "cairn-demo · Member". |
| Empty | "No projects" and "No projects for this account." No create or new-project button. |
| Delayed `GET /api/cairn/projects` | "Loading projects…" with `role="status"`, then the cards. The status was gone after the list arrived. |
| Intercepted 500 `{"error":"database unavailable"}` | Visible text "Projects could not be loaded. database unavailable". Removing the intercept showed the cards again. |
| No cookie, and a revoked session | Both `/projects` loads ended on Sign in. Upstream logout was 204. |
| `npm run typecheck` and `npm run lint` in `apps/web` | Passed. Production `next build` was not run. |

## Screenshots

| File | What it shows |
| --- | --- |
| `docs/audits/2026-11-12-projects-list/before/01-owner-1440.png` | Name-only link |
| `docs/audits/2026-11-12-projects-list/before/02-owner-390.png` | Same list at 390 |
| `docs/audits/2026-11-12-projects-list/before/03-owner-320.png` | Same list at 320 |
| `docs/audits/2026-11-12-projects-list/after/01-owner-1440.png` | Card links |
| `docs/audits/2026-11-12-projects-list/after/02-focus-1440.png` | Keyboard focus ring |
| `docs/audits/2026-11-12-projects-list/after/03-owner-390.png` | Wrapped long name |
| `docs/audits/2026-11-12-projects-list/after/04-owner-320.png` | Wrapped long name |
| `docs/audits/2026-11-12-projects-list/after/05-member-1440.png` | Member label |
| `docs/audits/2026-11-12-projects-list/after/06-empty-1440.png` | Empty account |
| `docs/audits/2026-11-12-projects-list/after/07-loading-1440.png` | Delayed loading |
| `docs/audits/2026-11-12-projects-list/after/08-error-1440.png` | Recoverable list failure |

## Limits

This does not certify WCAG, and DOM roles are not a screen-reader pass. A physical phone was not used. `next dev` still shows its own badge. Project detail, including its extra Projects link and the team 400 text, was not changed. The console still has no project-registration control, so the empty state does not offer one.
