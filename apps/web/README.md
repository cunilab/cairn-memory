# Cairn console

Next.js App Router console for the Cairn account and project API. The browser calls same-origin routes under `/api/cairn`. Those routes proxy to `CAIRN_API_URL` (default `http://127.0.0.1:3111`). Login stores the bearer token in an httpOnly cookie.

```bash
cd apps/web
npm install
CAIRN_API_URL=http://127.0.0.1:3111 npm run dev
```

`npm run typecheck` runs `tsc --noEmit`. The embedded Rust viewer at `/viewer` is unchanged.
