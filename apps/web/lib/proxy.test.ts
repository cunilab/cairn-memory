import assert from "node:assert/strict";
import { test } from "node:test";
import { ALLOWED, cookieMaxAge, forwardedHeaders, handleProxy, isNoBodyStatus, isSessionRequired, upstreamPath } from "./proxy.ts";

test("only allowlisted first segments pass", () => {
  for (const first of ALLOWED) assert.equal(upstreamPath([first]), first);
  for (const first of ["delete", "diagnose", "export", "observe", "remember", "livez", ""]) {
    assert.equal(upstreamPath([first]), null, first);
  }
  assert.equal(upstreamPath([]), null);
  assert.equal(upstreamPath(["stats", "attention"]), "stats/attention");
  assert.equal(upstreamPath(["console", "memories"]), "console/memories");
  assert.equal(upstreamPath(["console"]), null);
  assert.equal(upstreamPath(["console", "other"]), null);
  assert.equal(upstreamPath(["console", "memories", "extra"]), null);
});

test("dot and empty segments are refused after decoding", () => {
  assert.equal(upstreamPath(["memories", ".."]), null);
  assert.equal(upstreamPath(["memories", "."]), null);
  assert.equal(upstreamPath(["memories", ""]), null);
  assert.equal(upstreamPath(["auth", "..", "..", "etc"]), null);
});

test("a slash inside one segment stays inside it", () => {
  assert.equal(upstreamPath(["memories", "a/b"]), "memories/a%2Fb");
  assert.equal(upstreamPath(["projects", "x y", "branches"]), "projects/x%20y/branches");
  assert.equal(upstreamPath(["memories", "..%2F"]), "memories/..%252F");
});

test("the cookie lasts as long as the session, clamped at both ends", () => {
  assert.equal(cookieMaxAge(1100, 1000), 100);
  assert.equal(cookieMaxAge(900, 1000), 0);
  assert.equal(cookieMaxAge(1000, 1000), 0);
  assert.equal(cookieMaxAge(1000 + 60 * 60 * 24 * 90, 1000), 60 * 60 * 24 * 30);
});

test("session required is decided by the code, never the text", () => {
  assert.equal(isSessionRequired(401, "session_required"), true);
  assert.equal(isSessionRequired(401, "invalid_credentials"), false);
  assert.equal(isSessionRequired(401, "unauthorized"), false);
  assert.equal(isSessionRequired(403, "session_required"), false);
  assert.equal(isSessionRequired(401, undefined), false);
});

test("null-body statuses", () => {
  assert.equal(isNoBodyStatus(204), true);
  assert.equal(isNoBodyStatus(200), false);
});

// The route itself: a recording fetch stands in for the Cairn server.
type Call = { url: string; init: RequestInit };
function upstream(reply: () => Response) {
  const calls: Call[] = [];
  const fetchImpl = (async (url: URL, init: RequestInit) => {
    calls.push({ url: String(url), init });
    return reply();
  }) as unknown as typeof fetch;
  return { calls, fetchImpl };
}
const json = (body: unknown, status = 200) => Response.json(body, { status });
const sent = (call: Call) => new Headers(call.init.headers);
const API = "http://cairn-server:3111";

test("the session cookie becomes a bearer token and an incoming Authorization never passes", async () => {
  const { calls, fetchImpl } = upstream(() => json([]));
  const request = new Request("https://cairn.example.org/api/cairn/devices?x=1", {
    headers: { cookie: "other=1; cairn_token=sess%2Fabc", authorization: "Bearer cairn-device-stolen" },
  });
  const response = await handleProxy(request, ["devices"], API, fetchImpl);
  assert.equal(response.status, 200);
  assert.equal(calls[0].url, "http://cairn-server:3111/cairn-memory/devices?x=1");
  assert.equal(sent(calls[0]).get("authorization"), "Bearer sess/abc");
  const bare = await handleProxy(
    new Request("https://cairn.example.org/api/cairn/devices", { headers: { authorization: "Bearer cairn-device-stolen" } }),
    ["devices"],
    API,
    fetchImpl,
  );
  assert.equal(bare.status, 200);
  assert.equal(sent(calls[1]).get("authorization"), null);
});

test("the browser's origin reaches the server, so setup commands never show the internal host", async () => {
  const { calls, fetchImpl } = upstream(() => json({}));
  const request = new Request("http://cairn-web:3000/api/cairn/health", {
    headers: { host: "cairn-web:3000", "x-forwarded-host": "cairn.example.org", "x-forwarded-proto": "https" },
  });
  await handleProxy(request, ["health"], API, fetchImpl);
  const headers = sent(calls[0]);
  assert.equal(headers.get("x-forwarded-host"), "cairn.example.org");
  assert.equal(headers.get("x-forwarded-proto"), "https");
  assert.equal(headers.get("x-forwarded-prefix"), "/agent");
  // Without forwarding headers the request's own Host is used.
  const direct = forwardedHeaders(new Request("https://cairn.example.org/api/cairn/health", { headers: { host: "cairn.example.org" } }));
  assert.deepEqual(direct, { "x-forwarded-host": "cairn.example.org", "x-forwarded-proto": "https", "x-forwarded-prefix": "/agent" });
  // A local console has no public address to announce: the server's own is the one that works.
  for (const host of ["localhost:3000", "127.0.0.1:3000", "[::1]:3000"]) {
    assert.deepEqual(forwardedHeaders(new Request(`http://${host}/api/cairn/health`, { headers: { host } })), {}, host);
  }
});

test("login sets an httpOnly cookie and never hands the token to the page", async () => {
  const { fetchImpl } = upstream(() => json({ token: "tok-1", expires_at: 1100, person_id: "p" }));
  const request = new Request("https://cairn.example.org/api/cairn/auth/login", { method: "POST", body: "{}" });
  const response = await handleProxy(request, ["auth", "login"], API, fetchImpl, 1000);
  assert.deepEqual(await response.json(), { expires_at: 1100, person_id: "p" });
  assert.equal(response.headers.get("set-cookie"), "cairn_token=tok-1; Path=/; Max-Age=100; HttpOnly; SameSite=Lax; Secure");
  const plain = await handleProxy(
    new Request("http://127.0.0.1:3000/api/cairn/auth/bootstrap", { method: "POST", body: "{}" }),
    ["auth", "bootstrap"],
    API,
    fetchImpl,
    1000,
  );
  assert.doesNotMatch(plain.headers.get("set-cookie") ?? "", /Secure/);
});

test("a login reply without a token or expiry is refused", async () => {
  for (const body of [{ expires_at: 5 }, { token: "t" }]) {
    const { fetchImpl } = upstream(() => json(body));
    const response = await handleProxy(new Request("https://x.test/api/cairn/auth/login", { method: "POST", body: "{}" }), ["auth", "login"], API, fetchImpl);
    assert.equal(response.status, 502);
    assert.equal(response.headers.get("set-cookie"), null);
  }
});

test("register strips the new account's token and sets no cookie", async () => {
  const { fetchImpl } = upstream(() => json({ token: "other-session", person_id: "p2", email: "a@b.test" }));
  const request = new Request("https://x.test/api/cairn/auth/register", { method: "POST", body: "{}" });
  const response = await handleProxy(request, ["auth", "register"], API, fetchImpl);
  assert.deepEqual(await response.json(), { person_id: "p2", email: "a@b.test" });
  assert.equal(response.headers.get("set-cookie"), null);
});

test("logout clears the cookie when it worked or the session was already gone, not otherwise", async () => {
  const cases: [Response, boolean][] = [
    [new Response(null, { status: 204 }), true],
    [json({ code: "session_required" }, 401), true],
    [json({ code: "unauthorized" }, 401), false],
    [json({ code: "internal" }, 500), false],
  ];
  for (const [reply, cleared] of cases) {
    const { fetchImpl } = upstream(() => reply);
    const response = await handleProxy(new Request("https://x.test/api/cairn/auth/logout", { method: "POST", body: "{}" }), ["auth", "logout"], API, fetchImpl);
    assert.equal(response.headers.get("set-cookie"), cleared ? "cairn_token=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax; Secure" : null, String(reply.status));
  }
});

test("refused paths, an unreachable server and the next-page header", async () => {
  const { calls, fetchImpl } = upstream(() => new Response("[]", { headers: { "content-type": "application/json", "x-next-cursor": "c1", "set-cookie": "leak=1" } }));
  const refused = await handleProxy(new Request("https://x.test/api/cairn/export"), ["export"], API, fetchImpl);
  assert.equal(refused.status, 400);
  assert.equal(calls.length, 0);
  const listed = await handleProxy(new Request("https://x.test/api/cairn/auth/audit"), ["auth", "audit"], API, fetchImpl);
  assert.equal(listed.headers.get("x-next-cursor"), "c1");
  assert.equal(listed.headers.get("set-cookie"), null, "upstream headers are not copied wholesale");
  const down = (async () => {
    throw new Error("ECONNREFUSED");
  }) as unknown as typeof fetch;
  assert.equal((await handleProxy(new Request("https://x.test/api/cairn/health"), ["health"], API, down)).status, 502);
});

test("a body goes through on POST and a null-body status stays empty", async () => {
  const { calls, fetchImpl } = upstream(() => new Response(null, { status: 204 }));
  const request = new Request("https://x.test/api/cairn/devices/revoke", { method: "POST", body: '{"deviceId":"d"}', headers: { "content-type": "application/json" } });
  const response = await handleProxy(request, ["devices", "revoke"], API, fetchImpl);
  assert.equal(response.status, 204);
  assert.equal(calls[0].init.body, '{"deviceId":"d"}');
  assert.equal(sent(calls[0]).get("content-type"), "application/json");
});
