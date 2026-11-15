---
id: CM-AUDIT-001
title: "Desktop UI baseline — 2026-11-11"
status: draft
---

# Desktop UI baseline — 2026-11-11

Desktop Playwright baseline for the Next.js console. Mobile, a full accessibility audit, and production `next start` were not run.

## Setup

Two isolated file-store runs, both `next dev` at 1440×900.

| Run | API | Console | Fixture |
| --- | --- | --- | --- |
| First pass | `127.0.0.1:61701` | `127.0.0.1:61702` | `/tmp/cairn-ui-audit-20261111/store.json`. Owner `owner@example.test`, project `project-9b81a868135246d547726507`, one promoted decision, no team. |
| Gap pass | `127.0.0.1:62200` | `127.0.0.1:62201` | `/tmp/cairn-ui-audit-desktop2/store.json`. Same owner shape, plus `member@example.test` (invited, one shared decision, no team) and `empty@example.test` (no project). Project id `project-643321ce1ce1431e03f0b549`. |

The first pass covered login, one owner project, keyboard focus, log out, and a logged-out `/projects` visit. The gap pass did not repeat that owner login. It signed in only the empty and member accounts.

## Checklist

- [x] Login page, invalid password, and successful sign-in with Enter. First pass.
- [x] Owner projects list and keyboard activation of the project link. First pass.
- [x] Owner project detail, including the decision text and owner controls. First pass.
- [x] Tab order on login, projects, and owner project detail. Login email focus ring is visible.
- [x] Console and `/api/cairn` network log.
- [x] Log out attempted. Classified below. Not repeated on the gap pass.
- [x] Logged-out visit to `/projects`. Classified below.
- [x] Account with zero projects. Gap pass. Empty copy, keyboard, and a delayed loading state.
- [x] Member role. Gap pass. List, keyboard open, shared decision, no owner controls.
- [x] Unknown project id while signed in as the empty account.
- [ ] `next start` production build. Not run. Duplicate GETs were seen under `next dev` only.

## API contract versus UI

Direct API calls on the gap-pass store, then the same states in the browser.

| State | API | UI |
| --- | --- | --- |
| No session, `GET /projects` | 401 `{"error":"Bearer token is required"}`. The route requires a bearer. | Stays on `/projects`, prints that string, and still shows Log out. Presentation defect. Screenshot `01-projects-unauthenticated.png`. |
| Log out | API `POST /auth/logout` returns 204 with an empty body and revokes the session. First pass. | `apps/web/app/api/cairn/[...path]/route.ts:78` builds `new NextResponse(bytes, { status: 204 })`. Next throws `Invalid response status code 204`, returns 500, and does not clear `cairn_token`. The page stays and shows "Internal Server Error". The next `/projects` load sends the old cookie and the API answers "account session is expired or revoked". That 401 is the revoked-session contract. The 500 is the proxy. Screenshot `07-logout-error.png`. |
| No team, owner or member `GET /projects/{id}/team` | 400 `{"error":"validation error: project has no team space"}`. A reader does not receive an empty team. | The page prints that string in a red alert on an otherwise loaded project. Owner still gets the create-team form under it (`06-project-detail.png`). Member gets the alert and no owner form (`12-project-member.png`). The 400 is the contract. The raw alert is the presentation. |
| Zero projects | 200 `[]`. | "No projects for this account." No alert. Matches the empty list. Screenshot `08-projects-empty.png`. |
| Project id absent from that 200 list | The client does not call `/memories`, `/shared`, or `/team`. | Alert "Project is not on this account." Decisions and Team headings still render with nothing under them. Screenshot `10-project-not-on-account.png`. |
| Member read | `GET /projects` 200 with `role: "member"`. `GET .../shared` 200 with the one shared decision. No `GET .../memories`. | Shows `cairn-demo · member`, the shared decision, and `share_reason` `manual`. Share, revoke, delete, and invite are absent. Screenshot `12-project-member.png`. |

Console on the member detail recorded two 400s for `/team` and no other errors. `next dev` issued each GET twice. That duplicate was not checked with `next start`.

## Gap-pass behavior

Empty account, after sign-in: the main text is "Projects", "Log out", and "No projects for this account." Tab from the page background lands on Log out, then the Next.js dev-tools control. There is no project link. A fast local 200 did not leave "Loading projects…" on screen. Delaying `GET /api/cairn/projects` by 1.2 seconds made "Loading projects…" visible, then the empty copy replaced it. Screenshot `09-projects-loading.png`. No horizontal overflow at 1440px.

Opening `/projects/project-643321ce1ce1431e03f0b549` as the empty account shows the not-on-account alert. Tab order is the Projects link, then Log out.

Member account: Tab order is Log out, then the "Cairn Demo" link. Enter opens the detail. The decision text is the promoted session summary. Owner actions are not in the page. The team line is the 400 text above.

The round "N" control is the Next.js dev-tools indicator from `next dev`.

## Findings

1. **Log out fails in the console proxy after the API succeeds** (`apps/web/app/api/cairn/[...path]/route.ts:78`). Classified above. UI defect. The 204 is correct.

2. **A missing team is rendered as a page error.** `apps/web/components/project-detail.tsx:64-70` shows the 400 string in `role="alert"`. The API result is a validation error for both roles. The defect is presenting that expected absence as a failed load, including above the owner's create-team form.

3. **`/projects` without a session does not go to sign-in** (`apps/web/app/projects/page.tsx:23-44`). The 401 text is the API contract. Keeping the projects shell and a Log out button is the presentation defect.

4. **An unknown project id still shows Decisions and Team** (`apps/web/components/project-detail.tsx:149-247`). The list request succeeded and omitted the id. The headings render after the alert.

5. **Zero-project and member reads match the API.** Empty copy is right for `[]`. The member sees only the shared decision and no owner controls.

Invalid sign-in from the first pass shows "invalid account credentials" (`04-login-error.png`) with a console 401. That string is the API credential contract, shown as one alert. Successful owner Enter login reached the project list (`05-projects.png`).

## Screenshots

| File | What it shows |
| --- | --- |
| `docs/audits/2026-11-11-desktop-ui/01-projects-unauthenticated.png` | `/projects` with no session |
| `docs/audits/2026-11-11-desktop-ui/02-login.png` | Sign-in |
| `docs/audits/2026-11-11-desktop-ui/03-login-email-focus.png` | Email focus ring |
| `docs/audits/2026-11-11-desktop-ui/04-login-error.png` | Wrong password |
| `docs/audits/2026-11-11-desktop-ui/05-projects.png` | Owner, one project |
| `docs/audits/2026-11-11-desktop-ui/06-project-detail.png` | Owner decision, team 400, create-team form |
| `docs/audits/2026-11-11-desktop-ui/07-logout-error.png` | Log out 500 |
| `docs/audits/2026-11-11-desktop-ui/08-projects-empty.png` | Signed-in account with no projects |
| `docs/audits/2026-11-11-desktop-ui/09-projects-loading.png` | "Loading projects…" while the list GET was delayed |
| `docs/audits/2026-11-11-desktop-ui/10-project-not-on-account.png` | Unknown project id |
| `docs/audits/2026-11-11-desktop-ui/11-projects-member.png` | Member project list |
| `docs/audits/2026-11-11-desktop-ui/12-project-member.png` | Member shared decision and team 400 |

## Not run

Mobile, full accessibility, and `next start`. Memory, Rust, dead-code, and docs audits were out of scope.
