# Cairn console

Next.js App Router console for the Cairn account and project API. The browser calls same-origin routes under `/api/cairn`. Those routes proxy to `CAIRN_API_URL` (default `http://127.0.0.1:3111`). Login stores the bearer token in an httpOnly cookie.

```bash
cd apps/web
npm install
CAIRN_API_URL=http://127.0.0.1:3111 npm run dev
```

`npm run typecheck` runs `tsc --noEmit`. Wire types live in `lib/generated` (ts-rs; regenerate with `cargo test --workspace --features ts export_bindings`). The proxy forwards only the prefixes in `lib/proxy.ts`.

## Layout

Pages: `/dashboard`, `/memories`, `/activity`, `/projects`, `/graph`, `/settings` (tabs live in `?tab=`), and `/login`. The old `/devices`, `/accounts` and `/jobs` redirect to the matching tab.

Each page keeps its components in `app/<page>/_components` (helpers in `_lib`). `components/` holds only the shell and widgets used by several pages. The memory detail sheet is driven by `?memory=<id>` and mounted once in the shell, so any page can open it with `useOpenMemory()`.

`npm test` runs the `node:test` files.

