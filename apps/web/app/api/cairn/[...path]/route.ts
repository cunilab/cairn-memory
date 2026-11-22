import { NextRequest, NextResponse } from "next/server";
import type { ErrorBody } from "@/lib/generated/ErrorBody";
import type { Session } from "@/lib/generated/Session";
import { cookieMaxAge, isNoBodyStatus, isSessionRequired, upstreamPath } from "@/lib/proxy";

const COOKIE = "cairn_token";

type Context = { params: Promise<{ path: string[] }> };

function upstreamUrl(path: string, search: string): URL {
  const base = process.env.CAIRN_API_URL ?? "http://127.0.0.1:3111";
  const url = new URL(base);
  const prefix = url.pathname.replace(/\/$/, "");
  url.pathname = `${prefix}/cairn-memory/${path}`;
  url.search = search;
  return url;
}

function cookieOptions(request: NextRequest, maxAge: number) {
  return {
    httpOnly: true,
    sameSite: "lax" as const,
    path: "/",
    maxAge,
    secure: request.nextUrl.protocol === "https:",
  };
}

async function proxy(request: NextRequest, context: Context) {
  const { path } = await context.params;
  const target = upstreamPath(path);
  if (target === null) {
    return NextResponse.json({ error: "path not allowed", code: "path_not_allowed" }, { status: 400 });
  }
  const joined = path.join("/");
  const headers = new Headers();
  const contentType = request.headers.get("content-type");
  if (contentType) {
    headers.set("content-type", contentType);
  }
  const token = request.cookies.get(COOKIE)?.value;
  if (token) {
    headers.set("authorization", `Bearer ${token}`);
  }

  const rawBody =
    request.method === "GET" || request.method === "HEAD"
      ? undefined
      : await request.text();

  let upstream: Response;
  try {
    upstream = await fetch(upstreamUrl(target, request.nextUrl.search), {
      method: request.method,
      headers,
      body: rawBody ? rawBody : undefined,
      cache: "no-store",
    });
  } catch {
    return NextResponse.json({ error: "Cairn API is unreachable" }, { status: 502 });
  }

  const bytes = await upstream.arrayBuffer();

  if (
    request.method === "POST" &&
    (joined === "auth/login" || joined === "auth/bootstrap" || joined === "auth/register") &&
    upstream.ok
  ) {
    let payload: Partial<Session>;
    try {
      payload = JSON.parse(new TextDecoder().decode(bytes)) as Partial<Session>;
    } catch {
      return NextResponse.json({ error: "Login response was not JSON" }, { status: 502 });
    }
    // An admin creating an account keeps their own session and never receives the new token.
    if (joined === "auth/register") {
      delete payload.token;
      return NextResponse.json(payload, { status: upstream.status });
    }
    if (typeof payload.token !== "string" || payload.token.length === 0) {
      return NextResponse.json(
        { error: "Login response did not include a token" },
        { status: 502 },
      );
    }
    if (typeof payload.expires_at !== "number") {
      return NextResponse.json({ error: "Login response did not include an expiry" }, { status: 502 });
    }
    const sessionToken = payload.token;
    delete payload.token;
    const response = NextResponse.json(payload, { status: upstream.status });
    response.cookies.set(COOKIE, sessionToken, cookieOptions(request, cookieMaxAge(payload.expires_at, Math.floor(Date.now() / 1000))));
    return response;
  }

  // A null-body status rejects any body, including an empty buffer.
  const response = isNoBodyStatus(upstream.status)
    ? new NextResponse(null, { status: upstream.status })
    : new NextResponse(bytes, {
        status: upstream.status,
        headers: responseHeaders(upstream),
      });

  if (
    request.method === "POST" &&
    joined === "auth/logout" &&
    (upstream.ok || isSessionRequired(upstream.status, upstreamCode(bytes)))
  ) {
    response.cookies.set(COOKIE, "", cookieOptions(request, 0));
  }

  return response;
}

function responseHeaders(upstream: Response): Headers {
  const headers = new Headers({
    "content-type": upstream.headers.get("content-type") ?? "application/json",
  });
  // Paged lists carry their next cursor in a header.
  const cursor = upstream.headers.get("x-next-cursor");
  if (cursor) {
    headers.set("x-next-cursor", cursor);
  }
  return headers;
}

function upstreamCode(bytes: ArrayBuffer): unknown {
  if (bytes.byteLength === 0) {
    return undefined;
  }
  try {
    return (JSON.parse(new TextDecoder().decode(bytes)) as Partial<ErrorBody>).code;
  } catch {
    return undefined;
  }
}

export function GET(request: NextRequest, context: Context) {
  return proxy(request, context);
}

export function POST(request: NextRequest, context: Context) {
  return proxy(request, context);
}
