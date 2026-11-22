import assert from "node:assert/strict";
import { test } from "node:test";
import { ALLOWED, cookieMaxAge, isNoBodyStatus, isSessionRequired, upstreamPath } from "./proxy.ts";

test("only allowlisted first segments pass", () => {
  for (const first of ALLOWED) assert.equal(upstreamPath([first]), first);
  for (const first of ["delete", "diagnose", "export", "observe", "remember", "livez", ""]) {
    assert.equal(upstreamPath([first]), null, first);
  }
  assert.equal(upstreamPath([]), null);
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
