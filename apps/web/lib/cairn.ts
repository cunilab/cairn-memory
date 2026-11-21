import { isSessionRequired, SessionRequiredError } from "@/lib/session";

/** A failed API call. `status` lets a caller tell "not found" from "server down". */
export class ApiError extends Error {
  readonly status: number;
  constructor(message: string, status: number) {
    super(message);
    this.name = "ApiError";
    this.status = status;
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
    const message =
      body && typeof body === "object" && "error" in body
        ? String((body as { error: unknown }).error)
        : text || response.statusText;
    if (isSessionRequired(response.status, message)) {
      throw new SessionRequiredError(message);
    }
    throw new ApiError(message, response.status);
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
