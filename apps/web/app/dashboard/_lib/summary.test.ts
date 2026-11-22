import assert from "node:assert/strict";
import { test } from "node:test";
import { healthLabel, healthSegments, valueSentence } from "./summary.ts";

test("the value sentence is generated from numbers and marks the estimate", () => {
  const usage = { days: [], total_tokens: 24300, lookups: 812, hit_ratio: 0.94 };
  assert.equal(
    valueSentence(usage),
    `Your agents made ${(812).toLocaleString()} memory lookups in the last 30 days; 94% found something relevant. Cairn delivered ~${(24300).toLocaleString()} tokens of memory context.`,
  );
});

test("no usage reads as not yet, never as zero savings", () => {
  assert.match(valueSentence(undefined), /^No agent lookups yet\./);
  assert.match(valueSentence({ days: [], total_tokens: 0, lookups: 0, hit_ratio: 0 }), /^No agent lookups yet\./);
});

test("health segments keep a fixed order and drop empty ones", () => {
  const segments = healthSegments({ superseded: 2, active: 5, stale: 0 });
  assert.deepEqual(segments.map((s) => s.key), ["active", "superseded"]);
  assert.equal(healthLabel(segments), "Active 5, Superseded 2");
  assert.equal(healthLabel([]), "No memories yet");
});
