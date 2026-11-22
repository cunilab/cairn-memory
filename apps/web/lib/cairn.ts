import type { ErrorBody } from "@/lib/generated/ErrorBody";
import type { ErrorCode } from "@/lib/generated/ErrorCode";
import { isSessionRequired } from "@/lib/proxy";
import { SessionRequiredError } from "@/lib/session";

/** A failed API call. `status` tells "not found" from "server down"; `code` is the server's stable error code. */
export class ApiError extends Error {
  readonly status: number;
  readonly code: ErrorCode | null;
  constructor(message: string, status: number, code: ErrorCode | null = null) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }
}

async function request(path: string, init?: RequestInit): Promise<{ body: unknown; headers: Headers }> {
  const headers = new Headers(init?.headers);
  if (init?.body !== undefined && !headers.has("content-type")) {
    headers.set("content-type", "application/json");
  }

  const response = await fetch(`/api/cairn/${path.replace(/^\//, "")}`, {
    ...init,
    headers,
  });
  const text = await response.text();
  let body: unknown = null;
  if (text) {
    try {
      body = JSON.parse(text) as unknown;
    } catch {
      body = null;
    }
  }

  if (!response.ok) {
    const failure = body && typeof body === "object" && "error" in body ? (body as ErrorBody) : null;
    const message = failure ? String(failure.error) : text || response.statusText;
    if (isSessionRequired(response.status, failure?.code)) {
      throw new SessionRequiredError(message);
    }
    throw new ApiError(message, response.status, failure?.code ?? null);
  }

  return { body, headers: response.headers };
}

export async function cairn<T>(path: string, init?: RequestInit): Promise<T> {
  return (await request(path, init)).body as T;
}

/** A paged list: the next cursor comes from the `x-next-cursor` response header. */
export async function cairnPage<T>(path: string): Promise<{ items: T[]; next: string | null }> {
  const { body, headers } = await request(path);
  return { items: body as T[], next: headers.get("x-next-cursor") };
}
