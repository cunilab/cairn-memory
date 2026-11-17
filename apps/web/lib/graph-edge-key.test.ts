import assert from "node:assert/strict";
import { test } from "node:test";
import { graphEdgeKey } from "./graph-edge-key.ts";

test("colon-bearing memory ids do not share an edge key", () => {
  const left = graphEdgeKey("a", "relates", "b:causes:c");
  const right = graphEdgeKey("a:relates:b", "causes", "c");
  assert.notEqual(left, right);
  assert.equal(graphEdgeKey("a", "relates", "b"), graphEdgeKey("a", "relates", "b"));
});
