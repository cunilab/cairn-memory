import assert from "node:assert/strict";
import test from "node:test";
import { memoryLabel } from "./memory-label.ts";

const id = "mem_abcdefghijklmnopqrstuvwxyz";

test("active cards have no label", () => {
  assert.equal(memoryLabel({ id: "mem_a", status: "active", links: [] }), null);
});

test("a superseded card names the newest replacement", () => {
  const label = memoryLabel({
    id: "mem_old",
    status: "superseded",
    links: [
      { relation: "supersedes", source: "mem_older", target: "mem_old" },
      { relation: "supersedes", source: id, target: "mem_old" },
      { relation: "relates", source: "mem_other", target: "mem_old" },
    ],
  });
  assert.equal(label?.text, `replaced by ${id}`);
  assert.equal(label?.id, id);
  assert.ok((label?.text.length ?? 99) <= 42);
});

test("stale cards name the vote or the missing file", () => {
  const voted = memoryLabel({
    id: "mem_a",
    status: "stale",
    stale_cause: "feedback",
    wrong: 2,
    links: [],
  });
  const missing = memoryLabel({
    id: "mem_a",
    status: "stale",
    stale_cause: "missing",
    links: [],
  });
  assert.equal(voted?.text, "voted wrong 2x");
  assert.equal(missing?.text, "file deleted");
  assert.ok((voted?.text.length ?? 99) <= 16);
  assert.ok((missing?.text.length ?? 99) <= 16);
});
