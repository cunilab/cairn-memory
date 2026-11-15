---
id: CM-AUDIT-002
title: "shadcn/ui and Next.js research — 2026-11-11"
status: draft
---

# shadcn/ui and Next.js research — 2026-11-11

The installed console matches the current shadcn/ui Base UI setup. It does not need a library migration. The gaps are how a few screens use that setup.

## What was checked

Files: `apps/web/package.json`, `apps/web/package-lock.json` resolved versions, `apps/web/components.json`, `apps/web/app/globals.css`, `apps/web/app/layout.tsx`, `apps/web/app/page.tsx`, `apps/web/app/projects/page.tsx`, `apps/web/app/projects/[projectId]/page.tsx`, `apps/web/components/project-detail.tsx`, `apps/web/components/ui/button.tsx`, `apps/web/components/ui/input.tsx`, `apps/web/components/ui/label.tsx`, `apps/web/components/ui/card.tsx`, `apps/web/lib/utils.ts`, `apps/web/lib/cairn.ts`, `apps/web/app/api/cairn/[...path]/route.ts`.

No install, upgrade, lockfile change, or further browser pass. Lockfile versions: `next` 16.4.6, `react` 19.3.0, `tailwindcss` 4.3.3, `shadcn` 4.23.0, `@base-ui/react` 1.9.0, `cn` 0.5.2, `class-variance-authority` 0.7.1, `lucide-react` 1.72.0, `tw-animate-css` 1.4.0.

## Sources

- [shadcn/ui manual installation](https://ui.shadcn.com/docs/installation/manual), consulted 2026-11-11. Current install is Tailwind v4, `@import "shadcn/tailwind.css"`, `cn`, `class-variance-authority`, `lucide-react`, `tw-animate-css`, and the `shadcn` package.
- [shadcn/ui theming](https://ui.shadcn.com/docs/theming). `style: "base-nova"`, empty Tailwind config path, CSS variables, OKLCH tokens, `@theme inline`.
- [September 2026 — cn](https://ui.shadcn.com/docs/changelog/2026-09-cn). Components import `cn` from `cn`. `lib/utils.ts` re-exports it.
- [July 2026 — Base UI default](https://ui.shadcn.com/docs/changelog/2026-07-base-ui-default). New projects use `@base-ui/react`. Radix stays available, not required.
- [Field](https://ui.shadcn.com/docs/components/field) and [Next.js forms](https://ui.shadcn.com/docs/forms/next). Labels, `aria-invalid`, `FieldError`, and `useActionState` when a form uses a Server Action.
- [Next.js Server and Client Components](https://nextjs.org/docs/app/getting-started/server-and-client-components), updated 2026-10-05. Fetch and secrets stay on the server. `"use client"` wraps the interactive piece.

The button, input, and card files were not diffed against the live registry. Whether `@base-ui/react` 1.9.0 is the exact pin `shadcn@4.23.0` would install today is unverified. The July 2026 changelog mentioned Base UI 1.6.0; 1.9.0 is newer than that note.

## Verified match

`components.json` is the documented `base-nova` shape: `rsc: true`, blank `tailwind.config`, `baseColor: "neutral"`, `cssVariables: true`, `iconLibrary: "lucide"`. `app/globals.css` starts with the documented `@import "tailwindcss"`, `tw-animate-css`, and `shadcn/tailwind.css`, then OKLCH tokens and `@theme inline`. `lib/utils.ts` is the one-line `export { cn } from "cn"`. Button and input compose `@base-ui/react`, not Radix. The `shadcn` dependency is required by the CSS import. `lucide-react` is unused in source and is still the configured icon package. Leaving it installed matches the docs.

Copied component source is normal for shadcn. Card, button, input, and label being local files is not a violation.

## Gaps

### 1. Log out cannot return a 204 through this proxy

- **Evidence:** `apps/web/app/api/cairn/[...path]/route.ts:78`. Desktop pass: API logout is HTTP 204 with no body; the console route throws `Invalid response status code 204` and returns 500 without clearing `cairn_token`. The Rust session is already revoked.
- **Source:** the Next dev error names the `Response` constructor. A 204 response cannot carry a body ([MDN 204](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Status/204)).
- **Impact:** Log out looks failed, and the next request is an expired-session error.
- **Smallest fix:** for status 204, return `new NextResponse(null, { status: 204 })` and still clear the cookie. Do not set `content-type`.
- **Class:** correctness. Not a shadcn issue.

### 2. Authentication failure is a form-level error

- **Evidence:** `apps/web/app/page.tsx:51-83`. A rejected login renders one `role="alert"` under the fields. The API message is `invalid account credentials`, which names the pair, not the email or the password. The alert is not referenced from the `<form>` with `aria-describedby`. Owner-action failures in `apps/web/components/project-detail.tsx:274-354` are also one page-level alert.
- **Source:** [Field](https://ui.shadcn.com/docs/components/field) uses `aria-invalid` and `FieldError` when one control fails validation. [Next.js forms](https://ui.shadcn.com/docs/forms/next) do the same for a named field. That pattern is for field validation. It is a different case from an authentication result.
- **Impact:** The alert can be announced. It is not programmatically associated with the form. Marking both credential fields `aria-invalid` would say each field is invalid when the check only rejected the combination.
- **Smallest fix:** give the login alert an id and set `aria-describedby` on the `<form>` so the authentication error stays one form-level message. Set `aria-invalid` on a control only when validation names that control, such as a missing team name or a malformed member email. Do not mark both email and password for a failed sign-in. Add `field.tsx` only when a form is next edited for a real field error. Do not add Zod or React Hook Form for this.
- **Class:** accessibility recommendation. The missing form association is the gap. The absence of `aria-invalid` on both credential fields is not a defect.

### 3. Delete is an immediate button

- **Evidence:** `apps/web/components/project-detail.tsx:219-233`. The control calls `POST /api/cairn/delete` with no confirmation step in the component.
- **Source:** the current component index lists [Alert Dialog](https://ui.shadcn.com/docs/components/base/alert-dialog) for Base UI. The page body was not opened. Adding it is a registry copy, not a new primitive library.
- **Impact:** One click hides a memory. The desktop pass did not click it.
- **Smallest fix:** when that button is next changed, wrap it in the Base UI alert dialog from this same style. No other dialog work.
- **Class:** safety of a destructive control. Optional until that button is edited, then it should ship with the confirm.

## Recommendation, not a defect

Client fetch after paint is an optimization. `apps/web/app/projects/page.tsx:23-44` and `apps/web/components/project-detail.tsx:55-72` load data with `fetch` after mount through `apps/web/lib/cairn.ts:7`. The displayed projects matched the account. Login is a client page because it holds input state (`apps/web/app/page.tsx:17-39`). That interactivity is why the login page is a Client Component. It is not evidence that the project list must move to the server.

[Server and Client Components](https://nextjs.org/docs/app/getting-started/server-and-client-components) puts state and event handlers in a Client Component. A Server Component can fetch when that is useful. The docs do not make this same-origin client fetch a correctness failure. Under `next dev` the same GET ran twice. That duplicate was not checked with `next start`.

A later edit can load the project list in the server page and pass it to a client child that owns log out and links. Keep share, invite, and delete in the client detail. Leave the login form as a client form.

## Not a gap

- Radix is not missing. This style is Base UI on purpose.
- `cn` from the `cn` package is the current helper, not a leftover `clsx` wrapper.
- Local edits inside `components/ui` are allowed. shadcn components are owned source.
- Empty, Sidebar, Chart, and the rest of the catalog are unused because those screens do not exist. Do not add them in advance.
- `transition-all` on `apps/web/components/ui/button.tsx:6` was not compared to the current registry file. Leave it until a button diff says the registry changed.

## Implementation checklist

1. Fix the 204 proxy response and confirm log out clears `cairn_token` and lands on `/`.
2. Associate a failed login with the form as one error. Do not set `aria-invalid` on both credential fields unless validation names one field.
3. Optional later: load the project list in the server page. This is an optimization, not a required fix. Leave the login form as a client component.
4. On the next edit to delete, add the Base UI alert dialog and nothing else from the registry.
5. Do not change package versions or `components.json` `style` for this work.

## Blocked research

None of the cited doc pages were unreachable after a direct open. The installation overview at `https://ui.shadcn.com/docs/installation/next` returned no article body through the first fetch. The manual install and theming pages cover the same setup facts used above.
