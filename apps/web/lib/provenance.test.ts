import assert from "node:assert/strict";
import test from "node:test";
import { branchActivityHref, sessionHref, sessionTitle } from "./provenance.ts";

test("a verified session links to the inspector", () => {
  assert.equal(
    sessionHref({ person_id: "per_1", session_id: "s 1" }),
    "/session?person=per_1&session=s+1",
  );
  assert.equal(sessionTitle("main"), "Session on main");
  assert.equal(sessionTitle(null), "Session");
});

test("an unknown session opens branch activity without naming a session", () => {
  assert.equal(
    branchActivityHref("proj/1", "feat/x"),
    "/projects/proj%2F1?tab=sessions&branch=feat%2Fx",
  );
  assert.equal(branchActivityHref("proj", null), "/projects/proj?tab=sessions");
});
