// Pure proxy rules, kept free of Next so node:test can run them.
/** The first path segment the console may reach; a new top-level route the web needs goes here. */
export const ALLOWED = ["health", "auth", "devices", "admin", "projects", "memories", "graph", "stats", "forget", "agent-instructions", "skills"] as const;

const MAX_COOKIE_SECONDS = 60 * 60 * 24 * 30;
const NO_BODY_STATUSES = new Set([101, 204, 205, 304]);

/** The encoded upstream path, or null when the request must be refused (400). Segments arrive already decoded. */
export function upstreamPath(segments: string[]): string | null {
  if (segments[0] === "console") {
    return segments.length === 2 && segments[1] === "memories" ? "console/memories" : null;
  }
  if (segments[0] === "me") {
    return segments.length === 2 && segments[1] === "trace" ? "me/trace" : null;
  }
  if (segments.length === 0 || !(ALLOWED as readonly string[]).includes(segments[0])) return null;
  if (segments.some((s) => s === "" || s === "." || s === "..")) return null;
  return segments.map(encodeURIComponent).join("/");
}

/** The cookie lives as long as the server session: `expires_at` (epoch seconds) minus now, clamped. */
export function cookieMaxAge(expiresAt: number, nowSeconds: number): number {
  return Math.max(0, Math.min(expiresAt - nowSeconds, MAX_COOKIE_SECONDS));
}

export function isSessionRequired(status: number, code: unknown): boolean {
  return status === 401 && code === "session_required";
}

export function isNoBodyStatus(status: number): boolean {
  return NO_BODY_STATUSES.has(status);
}

const COOKIE = "cairn_token";
/** Where the deploy layout (apps/docs/content/technical/deployment.md) routes agents on the console's own host. */
const AGENT_PREFIX = "/agent";

const first = (value: string | null): string | null => value?.split(",")[0].trim() || null;

const isLoopback = (host: string): boolean => /^(localhost|127\.\d+\.\d+\.\d+|\[::1\])(:\d+)?$/.test(host);

/**
 * The address the browser used, for the server's `public_url` when `CAIRN_PUBLIC_URL` is unset.
 * A public host gets the `/agent` prefix the reverse proxy strips; a loopback console is a dev
 * setup where the server's own address is the one agents can reach, so nothing is forwarded.
 */
export function forwardedHeaders(request: Request): Record<string, string> {
  const url = new URL(request.url);
  const host = first(request.headers.get("x-forwarded-host")) ?? first(request.headers.get("host")) ?? url.host;
  if (isLoopback(host)) return {};
  const proto = first(request.headers.get("x-forwarded-proto")) ?? url.protocol.replace(":", "");
  return { "x-forwarded-host": host, "x-forwarded-proto": proto, "x-forwarded-prefix": AGENT_PREFIX };
}

function sessionCookie(value: string, maxAge: number, secure: boolean): string {
  return `${COOKIE}=${encodeURIComponent(value)}; Path=/; Max-Age=${maxAge}; HttpOnly; SameSite=Lax${secure ? "; Secure" : ""}`;
}

function readCookie(request: Request): string | undefined {
  for (const part of (request.headers.get("cookie") ?? "").split(";")) {
    const [name, ...rest] = part.trim().split("=");
    if (name === COOKIE) return decodeURIComponent(rest.join("="));
  }
  return undefined;
}

function upstreamUrl(apiUrl: string, path: string, search: string): URL {
  const url = new URL(apiUrl);
  url.pathname = `${url.pathname.replace(/\/$/, "")}/cairn-memory/${path}`;
  url.search = search;
  return url;
}

function upstreamCode(bytes: ArrayBuffer): unknown {
  if (bytes.byteLength === 0) return undefined;
  try {
    return (JSON.parse(new TextDecoder().decode(bytes)) as { code?: unknown }).code;
  } catch {
    return undefined;
  }
}

function responseHeaders(upstream: Response): Headers {
  const headers = new Headers({ "content-type": upstream.headers.get("content-type") ?? "application/json" });
  // Paged lists carry their next cursor in a header.
  const cursor = upstream.headers.get("x-next-cursor");
  if (cursor) headers.set("x-next-cursor", cursor);
  return headers;
}

/**
 * The console's `/api/cairn/*` route: browser to server, with the session cookie turned into a
 * bearer token. An incoming Authorization header is never forwarded, so a device token cannot
 * ride the console. Plain Request and Response, so node:test can run it.
 */
export async function handleProxy(
  request: Request,
  path: string[],
  apiUrl: string,
  fetchImpl: typeof fetch = fetch,
  nowSeconds: number = Math.floor(Date.now() / 1000),
): Promise<Response> {
  const target = upstreamPath(path);
  if (target === null) {
    return Response.json({ error: "path not allowed", code: "path_not_allowed" }, { status: 400 });
  }
  const joined = path.join("/");
  const headers = new Headers(forwardedHeaders(request));
  const contentType = request.headers.get("content-type");
  if (contentType) headers.set("content-type", contentType);
  const token = readCookie(request);
  if (token) headers.set("authorization", `Bearer ${token}`);
  const secure = (first(request.headers.get("x-forwarded-proto")) ?? new URL(request.url).protocol.replace(":", "")) === "https";

  const rawBody = request.method === "GET" || request.method === "HEAD" ? undefined : await request.text();
  let upstream: Response;
  try {
    upstream = await fetchImpl(upstreamUrl(apiUrl, target, new URL(request.url).search), {
      method: request.method,
      headers,
      body: rawBody ? rawBody : undefined,
      cache: "no-store",
    });
  } catch {
    return Response.json({ error: "Cairn API is unreachable" }, { status: 502 });
  }
  const bytes = await upstream.arrayBuffer();

  if (
    request.method === "POST" &&
    (joined === "auth/login" || joined === "auth/bootstrap" || joined === "auth/register") &&
    upstream.ok
  ) {
    let payload: { token?: unknown; expires_at?: unknown };
    try {
      payload = JSON.parse(new TextDecoder().decode(bytes));
    } catch {
      return Response.json({ error: "Login response was not JSON" }, { status: 502 });
    }
    // An admin creating an account keeps their own session and never receives the new token.
    if (joined === "auth/register") {
      delete payload.token;
      return Response.json(payload, { status: upstream.status });
    }
    if (typeof payload.token !== "string" || payload.token.length === 0) {
      return Response.json({ error: "Login response did not include a token" }, { status: 502 });
    }
    if (typeof payload.expires_at !== "number") {
      return Response.json({ error: "Login response did not include an expiry" }, { status: 502 });
    }
    const sessionToken = payload.token;
    delete payload.token;
    return Response.json(payload, {
      status: upstream.status,
      headers: { "set-cookie": sessionCookie(sessionToken, cookieMaxAge(payload.expires_at, nowSeconds), secure) },
    });
  }

  // A null-body status rejects any body, including an empty buffer.
  const response = isNoBodyStatus(upstream.status)
    ? new Response(null, { status: upstream.status })
    : new Response(bytes, { status: upstream.status, headers: responseHeaders(upstream) });
  if (
    request.method === "POST" &&
    joined === "auth/logout" &&
    (upstream.ok || isSessionRequired(upstream.status, upstreamCode(bytes)))
  ) {
    response.headers.append("set-cookie", sessionCookie("", 0, secure));
  }
  return response;
}
