/** Where a missing or ended session sends the person. */
export const LOGIN_PATH = "/login";

const NO_BODY_STATUSES = new Set([101, 204, 205, 304]);

const SESSION_REQUIRED_MESSAGES = new Set([
  "Bearer token is required",
  "authentication failed",
  "account authentication failed",
  "account session is expired or revoked",
]);

export function isNoBodyStatus(status: number): boolean {
  return NO_BODY_STATUSES.has(status);
}

export function isSessionRequired(status: number, message: string): boolean {
  return status === 401 && SESSION_REQUIRED_MESSAGES.has(message);
}

export function signInFailureMessage(message: string): string {
  if (message === "invalid account credentials") {
    return "Email or password is incorrect.";
  }
  return message;
}

export class SessionRequiredError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "SessionRequiredError";
  }
}
