---
id: CM-AUDIT-004
title: "Sign-in flow task 1 — 2026-11-12"
status: draft
---

# Sign-in flow task 1 — 2026-11-12

Evidence for the authentication flow and sign-in screen. Project-list polish, the team empty state, memory-card wrapping, and tap targets are not part of this pass.

## Sources

These pages were opened in a browser or returned a registry body on 2026-11-12.

| Source | What was available |
| --- | --- |
| [Next.js installation](https://ui.shadcn.com/docs/installation/next) | The article loaded. An existing Next.js app is configured in place. It is not a new `shadcn init`. Local `shadcn info --json` reports Next.js 16.4.6, Tailwind v4, `base-nova`, Base UI, and Lucide. |
| [Skills](https://ui.shadcn.com/docs/skills) | The article loaded. It says forms use `FieldGroup` and the assistant should read `components.json`. This pass added `field.tsx` because login-04 uses it. |
| [Blocks](https://ui.shadcn.com/blocks) | The page listed dashboard-01, login-03, and login-04. |
| [login-04 registry](https://ui.shadcn.com/r/styles/base-nova/login-04.json) | Split card, email, password, and a side panel. The registry also includes Apple, Google, Meta, a forgot-password link, sign-up, and terms. Those controls are not in the product, so they were not copied. |
| Fetch `Response` constructor in `undici` | Statuses 101, 204, 205, and 304 reject a body. An empty `ArrayBuffer` still counts as a body. |

## What changed

- `POST /api/cairn/auth/logout` returns the upstream 204 with no body and clears the httpOnly `cairn_token` cookie. A session-required 401 on logout also clears that cookie. Other statuses, including a failed sign-in, still return their upstream body.
- A missing bearer, a bad account token, or an expired or revoked session throws `SessionRequiredError`. The projects list and project detail then go to `/` and do not keep the raw 401 or owner controls. `invalid account credentials` stays on the form.
- Sign-in uses the login-04 split card and `Field` layout. The copy is Cairn's. The side panel is text, not `/placeholder.svg`. `/login` redirects to `/`.
- Signed-in projects and project detail use the dashboard-01 sidebar shell with one Projects link and Log out. `/dashboard` redirects to `/projects`. The demo chart, data table, and their extra packages were not kept.

## Checks

| Check | Result |
| --- | --- |
| `./scripts/smoke_web.sh` | Passed. It includes the session assertions, a 401 credential login that does not set `cairn_token`, a 204 logout with an empty body and an httpOnly removal cookie, a revoked-token 401 that still says `account session is expired or revoked`, and a signed-out projects request that still says `Bearer token is required`. |
| `npm run typecheck` and `npm run lint` in `apps/web` | Passed. |
| `npm run build` | Passed. Production `next start` on `127.0.0.1:65251` against the isolated audit API. |
| Playwright on `next dev`, 1440, 390, and 320 | Sign-in, invalid sign-in, successful sign-in, logout, and a direct protected URL. No horizontal overflow. Email text is 16px at 390 and 320. Focused email uses a 3px ring. |
| Playwright on `next start`, 1440 | Invalid login 401, login 200, projects 200, logout 204, cookie absent, `/projects` and a project URL return to Sign in. No Next.js dev-tools button. |
| Revoked session on `next dev` | Upstream logout returned 204. Reloading the open project landed on Sign in. Share and Log out were not visible. The raw revoked-session string was not shown. |

Invalid sign-in shows "Email or password is incorrect." The form's `aria-describedby` is `sign-in-error`. Email and password do not get `aria-invalid`.

The browser 401 on a bad password or a signed-out projects fetch is the expected network failure. It is not an application exception.

## Screenshots

Before, from the unfixed console: `docs/audits/2026-11-12-signin/before/`.

After: `docs/audits/2026-11-12-signin/after/`. `09-login-production-1440.png` is the production sign-in page. The earlier 2026-11-11 audit screenshots were left in place.

## Limits

The team request still returns 400 `project has no team space`, and the detail page still shows that string. Memory cards and project-link hit targets were not changed. A physical phone was not used. This note does not say the rest of the console is fixed.
