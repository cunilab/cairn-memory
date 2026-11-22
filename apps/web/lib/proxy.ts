// Pure proxy rules, kept free of Next so node:test can run them.
/** The first path segment the console may reach; a new top-level route the web needs goes here. */
export const ALLOWED = ["health", "auth", "devices", "admin", "projects", "memories", "graph", "stats", "forget"] as const;

const MAX_COOKIE_SECONDS = 60 * 60 * 24 * 30;
const NO_BODY_STATUSES = new Set([101, 204, 205, 304]);

/** The encoded upstream path, or null when the request must be refused (400). Segments arrive already decoded. */
export function upstreamPath(segments: string[]): string | null {
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
