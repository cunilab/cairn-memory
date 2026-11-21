import assert from "node:assert/strict";
import { test } from "node:test";
import { auditLabel, dayKey, dayLabel, describeEvent, isSystemAction, type AuditEvent } from "./events.ts";

const event = (action: string, metadata: Record<string, unknown> | null = {}, extra: Partial<AuditEvent> = {}): AuditEvent => ({
  audit_id: "a",
  action,
  subject_id: null,
  device_id: null,
  metadata,
  created_at: 0,
  ...extra,
});

test("system events read as Cairn did it, counts first", () => {
  assert.equal(
    describeEvent(event("memory_merged", { into: "m1", merged: ["a", "b"] }, { titles: { m1: "Migrations must be additive" } })),
    'Cairn merged 2 duplicate memories into "Migrations must be additive"',
  );
  assert.equal(describeEvent(event("links_added", { count: 1, entity: "src/money.rs" })), "Cairn linked 1 memory that cites src/money.rs");
  assert.equal(describeEvent(event("repo_state", { stale: 3, revived: 1 })), "Cairn marked 3 stale, revived 1");
  assert.equal(describeEvent(event("memories_expired", { count: 5, by_kind: { note: 5 } })), "Cairn expired 5 memories (5 note)");
  assert.equal(describeEvent(event("session_distilled", { branch: "feat/x", notes: 2 })), "Cairn wrote a handoff for feat/x and saved 2 notes");
  assert.equal(describeEvent(event("job_quarantined", { kind: "link", attempts: 5, error: "boom" })), "A link job stopped after 5 tries: boom");
});

test("missing metadata never throws and unknown actions stay visible", () => {
  assert.equal(describeEvent(event("repo_state", null)), "Cairn checked memories against the repository");
  assert.equal(describeEvent(event("something_new")), "Cairn: something_new");
});

test("audit labels are human and fall back to the action", () => {
  assert.equal(auditLabel("device_registered"), "Created a device token");
  assert.equal(auditLabel("custom_thing"), "Custom thing");
  assert.equal(isSystemAction("repo_state"), true);
  assert.equal(isSystemAction("login"), false);
});

test("days are labelled relative to now", () => {
  const now = Math.floor(new Date(2026, 10, 22, 12).getTime() / 1000);
  assert.equal(dayLabel(dayKey(now), now), "Today");
  assert.equal(dayLabel(dayKey(now - 86400), now), "Yesterday");
});
