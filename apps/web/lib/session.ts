import type { ErrorCode } from "@/lib/generated/ErrorCode";

/** Where a missing or ended session sends the person. */
export const LOGIN_PATH = "/login";

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
