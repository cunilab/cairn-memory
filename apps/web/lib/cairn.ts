import { isSessionRequired, SessionRequiredError } from "@/lib/session";

export async function cairn<T>(path: string, init?: RequestInit): Promise<T> {
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
    throw new Error(message);
  }

  return body as T;
}
