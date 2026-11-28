import type { ErrorCode } from "@/lib/generated/ErrorCode";

/** Where a missing or ended session sends the person. */
export const LOGIN_PATH = "/login";

/** Sign-in URL that returns to the current same-origin path, including filters. */
export function loginHref(): string {
  if (typeof window === "undefined") return LOGIN_PATH;
  const next = `${window.location.pathname}${window.location.search}`;
  if (!next.startsWith("/") || next.startsWith("//") || next.startsWith("/login")) return LOGIN_PATH;
  return `${LOGIN_PATH}?next=${encodeURIComponent(next)}`;
}

/** A validated same-origin path to open after sign-in. */
export function safeNext(value: string | null): string | null {
  if (!value || !value.startsWith("/") || value.startsWith("//") || value.startsWith("/login")) return null;
  return value;
}

/** The sign-in form's message for a failed attempt; other failures keep the server's text. */
export function signInFailureMessage(code: ErrorCode | null, message: string): string {
  return code === "invalid_credentials" ? "Email or password is incorrect." : message;
}

export class SessionRequiredError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "SessionRequiredError";
  }
}
