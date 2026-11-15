import { NextRequest, NextResponse } from "next/server";
import { isNoBodyStatus, isSessionRequired } from "@/lib/session";

const COOKIE = "cairn_token";

type Context = { params: Promise<{ path: string[] }> };

function upstreamUrl(path: string[], search: string): URL {
  const base = process.env.CAIRN_API_URL ?? "http://127.0.0.1:3111";
  const url = new URL(base);
  const prefix = url.pathname.replace(/\/$/, "");
  url.pathname = `${prefix}/cairn-memory/${path.map((segment) => encodeURIComponent(segment)).join("/")}`;
  url.search = search;
  return url;
}

function cookieOptions(request: NextRequest) {
  return {
    httpOnly: true,
    sameSite: "lax" as const,
    path: "/",
    secure: request.nextUrl.protocol === "https:",
  };
}

async function proxy(request: NextRequest, context: Context) {
  const { path } = await context.params;
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
    upstream = await fetch(upstreamUrl(path, request.nextUrl.search), {
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
    (joined === "auth/login" ||
      joined === "auth/bootstrap" ||
      joined === "auth/register") &&
    upstream.ok
  ) {
    let payload: { token?: unknown };
    try {
      payload = JSON.parse(new TextDecoder().decode(bytes)) as { token?: unknown };
    } catch {
      return NextResponse.json({ error: "Login response was not JSON" }, { status: 502 });
    }
    if (typeof payload.token !== "string" || payload.token.length === 0) {
      return NextResponse.json(
        { error: "Login response did not include a token" },
        { status: 502 },
      );
    }
    const sessionToken = payload.token;
    delete payload.token;
    const response = NextResponse.json(payload, { status: upstream.status });
    response.cookies.set(COOKIE, sessionToken, cookieOptions(request));
    return response;
  }

  // A null-body status rejects any body, including an empty buffer.
  const response = isNoBodyStatus(upstream.status)
    ? new NextResponse(null, { status: upstream.status })
    : new NextResponse(bytes, {
        status: upstream.status,
        headers: {
          "content-type": upstream.headers.get("content-type") ?? "application/json",
        },
      });

  if (
    request.method === "POST" &&
    joined === "auth/logout" &&
    (upstream.ok || isSessionRequired(upstream.status, upstreamError(bytes)))
  ) {
    response.cookies.set(COOKIE, "", { ...cookieOptions(request), maxAge: 0 });
  }

  return response;
}

function upstreamError(bytes: ArrayBuffer): string {
  if (bytes.byteLength === 0) {
    return "";
  }
  try {
    const payload = JSON.parse(new TextDecoder().decode(bytes)) as { error?: unknown };
    return typeof payload.error === "string" ? payload.error : "";
  } catch {
    return "";
  }
}

export function GET(request: NextRequest, context: Context) {
  return proxy(request, context);
}

export function POST(request: NextRequest, context: Context) {
  return proxy(request, context);
}
