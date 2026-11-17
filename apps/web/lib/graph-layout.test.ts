import assert from "node:assert/strict";
import { test } from "node:test";
import { NODE_SIZE, fitBBox, layoutGraph } from "./graph-layout.ts";

function chain(count: number, prefix: string) {
  const ids = Array.from({ length: count }, (_, index) => `${prefix}${index}`);
  const edges = ids.slice(1).map((id, index) => ({ source: ids[index], target: id }));
  return { ids, edges };
}

function gaps(positions: Map<string, { x: number; y: number }>) {
  const points = [...positions.values()];
  let min = Infinity;
  for (let i = 0; i < points.length; i += 1) {
    for (let j = i + 1; j < points.length; j += 1) {
      min = Math.min(min, Math.hypot(points[i].x - points[j].x, points[i].y - points[j].y));
    }
  }
  return min;
}

function extent(positions: Map<string, { x: number; y: number }>) {
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const point of positions.values()) {
    minX = Math.min(minX, point.x);
    minY = Math.min(minY, point.y);
    maxX = Math.max(maxX, point.x);
    maxY = Math.max(maxY, point.y);
  }
  return { width: maxX - minX, height: maxY - minY };
}

test("a chain keeps a gap between nodes and opens at the ends", () => {
  const { ids, edges } = chain(12, "n");
  const { positions } = layoutGraph(ids, edges);
  const min = gaps(positions);
  assert.ok(min >= NODE_SIZE * 2, `nodes overlap at ${min}`);
  let minEdge = Infinity;
  for (let index = 0; index < ids.length - 1; index += 1) {
    const left = positions.get(ids[index]);
    const right = positions.get(ids[index + 1]);
    assert.ok(left && right);
    minEdge = Math.min(minEdge, Math.hypot(left.x - right.x, left.y - right.y));
  }
  const start = positions.get(ids[0]);
  const end = positions.get(ids[ids.length - 1]);
  assert.ok(start && end);
  const endGap = Math.hypot(start.x - end.x, start.y - end.y);
  assert.ok(endGap > minEdge * 2, `chain closed up (${endGap} vs edge ${minEdge})`);
});

test("five hundred chain nodes stay apart and are not a thin pillar", () => {
  const { ids, edges } = chain(500, "g");
  const { positions, milliseconds } = layoutGraph(ids, edges);
  const min = gaps(positions);
  assert.ok(min >= NODE_SIZE * 2, `nodes overlap at ${min}`);
  const box = extent(positions);
  const aspect = Math.max(box.width, box.height) / Math.max(1, Math.min(box.width, box.height));
  assert.ok(aspect < 4, `layout collapsed into a pillar (${aspect})`);
  assert.ok(
    Math.max(box.width, box.height) < 1_200,
    `chain spread too far to read when fitted (${box.width}x${box.height})`,
  );
  assert.ok(milliseconds < 400, `layout took ${milliseconds}ms`);
  const again = layoutGraph(ids, edges);
  for (const id of ids) {
    const left = positions.get(id);
    const right = again.positions.get(id);
    assert.ok(left && right);
    assert.equal(left.x, right.x);
    assert.equal(left.y, right.y);
  }
});

test("a linked pair stays apart on a horizontal edge", () => {
  const { positions } = layoutGraph(["b", "a"], [{ source: "b", target: "a" }]);
  const left = positions.get("a");
  const right = positions.get("b");
  assert.ok(left && right);
  assert.equal(left.y, right.y);
  assert.ok(left.x < right.x);
  assert.ok(right.x - left.x >= NODE_SIZE * 4);
});

test("disconnected components are packed without stacking", () => {
  const first = chain(6, "a");
  const second = chain(6, "b");
  const ids = [...first.ids, ...second.ids, "alone"];
  const { positions } = layoutGraph(ids, [...first.edges, ...second.edges]);
  const min = gaps(positions);
  assert.ok(min >= NODE_SIZE * 2, `components overlap at ${min}`);
  const alone = positions.get("alone");
  const other = positions.get("a0");
  assert.ok(alone && other);
  assert.ok(Math.hypot(alone.x - other.x, alone.y - other.y) > NODE_SIZE * 4);
});

test("a sparse layout is capped so fitted nodes stay a readable size", () => {
  const wide = fitBBox(
    [
      { x: 0, y: 0 },
      { x: 32, y: 0 },
    ],
    { width: 1440, height: 800 },
    48,
  );
  assert.ok(wide);
  const fitted = NODE_SIZE * Math.min((1440 - 96) / (wide.x[1] - wide.x[0]), (800 - 96) / (wide.y[1] - wide.y[0]));
  assert.ok(fitted <= 22.01, `capped radius ${fitted}`);
  assert.equal(
    fitBBox(
      [
        { x: 0, y: 0 },
        { x: 8_000, y: 6_000 },
      ],
      { width: 1440, height: 800 },
      48,
    ),
    null,
  );
});
