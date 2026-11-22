import assert from "node:assert/strict";
import { test } from "node:test";
import { baselineOf, checkedInSince } from "./device-checkin.ts";

const row = (last_seen_at: number, last_hook_at: number | null) => ({ last_seen_at, last_hook_at });

test("a new device is connected by its first hook or any use of the token", () => {
  const base = baselineOf(row(1000, null));
  assert.equal(checkedInSince(row(1000, null), base), false);
  assert.equal(checkedInSince(row(1005, null), base), true);
  assert.equal(checkedInSince(row(1000, 1004), base), true);
});

test("a rotated device that connected before is not connected until the new token is used", () => {
  // Rotation keeps last_hook_at and moves last_seen_at to the rotation time.
  const base = baselineOf(row(5000, 4000));
  assert.equal(checkedInSince(row(5000, 4000), base), false, "the old check-in is not this one");
  assert.equal(checkedInSince(row(5003, 4000), base), true, "the machine used the new token");
  assert.equal(checkedInSince(row(5000, 5002), base), true, "a hook ran with the new token");
});
