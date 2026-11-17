import Graph from "graphology";
import forceAtlas2 from "graphology-layout-forceatlas2";

/** Radius in graph units. Sigma draws this with `itemSizesReference: "positions"`. */
export const NODE_SIZE = 8;

const SPACING = NODE_SIZE * 4;
const COMPONENT_GAP = NODE_SIZE * 6;
/** Fitted radius above this, in pixels, means a sparse graph would become giant disks. */
const MAX_FITTED_RADIUS = 22;
/** A path longer than this is folded into a block so a fitted view keeps nodes visible. */
const PACKED_PATH = 40;

// graphology-layout-forceatlas2 0.10.1 only applies size-aware repulsion on the
// direct pass. Barnes–Hut ignores `adjustSizes`, so a long chain collapses into a stack.
const SETTINGS = {
  gravity: 0.02,
  scalingRatio: 14,
  slowDown: 1,
  adjustSizes: true,
  strongGravityMode: false,
  barnesHutOptimize: false,
  outboundAttractionDistribution: false,
} as const;

export type LayoutEdge = { source: string; target: string };

export type LayoutSession = {
  /** One bounded slice. True when every component has finished. */
  step(): boolean;
  positions(): Map<string, { x: number; y: number }>;
  /** ForceAtlas2 time in milliseconds, excluding the waits between slices. */
  milliseconds(): number;
};

function iterationBudget(order: number): number {
  if (order < 2) return 0;
  return order <= 40 ? 80 : 60;
}

function neighborIds(graph: Graph, id: string): string[] {
  const found = new Set<string>();
  graph.forEachNeighbor(id, (neighbor) => {
    found.add(neighbor);
  });
  return [...found].sort();
}

/** Walk a path from its lower endpoint so a chain starts as a chain. Other graphs stay in id order. */
function orderComponent(graph: Graph, ids: string[]): string[] {
  const neighbors = new Map(ids.map((id) => [id, neighborIds(graph, id)]));
  const ends = ids.filter((id) => neighbors.get(id)?.length === 1).sort();
  const branched = ids.some((id) => (neighbors.get(id)?.length ?? 0) > 2);
  if (branched || ends.length !== 2) return [...ids].sort();
  const ordered: string[] = [];
  const seen = new Set<string>();
  let current: string | undefined = ends[0];
  while (current && !seen.has(current)) {
    ordered.push(current);
    seen.add(current);
    current = neighbors.get(current)?.find((next) => !seen.has(next));
  }
  return ordered.length === ids.length ? ordered : [...ids].sort();
}

function placeOnArc(index: number, count: number): { x: number; y: number } {
  if (count < 2) return { x: 0, y: 0 };
  const steps = count - 1;
  const span = Math.min(Math.PI * 1.5, steps * 0.55);
  const step = span / steps;
  const radius = SPACING / (2 * Math.sin(step / 2));
  const angle = -span / 2 + step * index;
  return { x: Math.cos(angle) * radius, y: Math.sin(angle) * radius };
}

/** Fold a long path into a serpentine block. Adjacent chain nodes stay one `SPACING` apart. */
function placePackedPath(index: number, count: number): { x: number; y: number } {
  const columns = Math.max(2, Math.ceil(Math.sqrt(count)));
  const row = Math.floor(index / columns);
  const offset = index % columns;
  const column = row % 2 === 0 ? offset : columns - 1 - offset;
  return { x: column * SPACING, y: row * SPACING };
}

function linked(graph: Graph, from: string, to: string): boolean {
  return graph.hasDirectedEdge(from, to) || graph.hasDirectedEdge(to, from);
}

function openPath(graph: Graph, ordered: string[]): boolean {
  if (ordered.length <= PACKED_PATH) return false;
  for (let index = 1; index < ordered.length; index += 1) {
    if (!linked(graph, ordered[index - 1], ordered[index])) return false;
  }
  return true;
}

/**
 * Bounding box that keeps a fitted node at most `MAX_FITTED_RADIUS` pixels.
 * Returns null when the natural fit is already that small or smaller.
 */
export function fitBBox(
  points: Iterable<{ x: number; y: number }>,
  viewport: { width: number; height: number },
  padding: number,
): { x: [number, number]; y: [number, number] } | null {
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  let count = 0;
  for (const point of points) {
    minX = Math.min(minX, point.x);
    minY = Math.min(minY, point.y);
    maxX = Math.max(maxX, point.x);
    maxY = Math.max(maxY, point.y);
    count += 1;
  }
  if (count === 0 || viewport.width < 2 || viewport.height < 2) return null;
  const extentW = Math.max(maxX - minX, NODE_SIZE * 2);
  const extentH = Math.max(maxY - minY, NODE_SIZE * 2);
  const innerW = Math.max(viewport.width - padding * 2, 1);
  const innerH = Math.max(viewport.height - padding * 2, 1);
  const fittedRadius = NODE_SIZE * Math.min(innerW / extentW, innerH / extentH);
  if (fittedRadius <= MAX_FITTED_RADIUS) return null;
  const scale = fittedRadius / MAX_FITTED_RADIUS;
  const boxW = extentW * scale;
  const boxH = extentH * scale;
  const centerX = (minX + maxX) / 2;
  const centerY = (minY + maxY) / 2;
  return {
    x: [centerX - boxW / 2, centerX + boxW / 2],
    y: [centerY - boxH / 2, centerY + boxH / 2],
  };
}

type ComponentLayout = { graph: Graph; ids: string[]; remaining: number };

/** A linked pair has no preferred direction. Keep the force distance and lay the edge left to right. */
function levelPair(graph: Graph, ids: string[]) {
  if (ids.length !== 2) return;
  const [left, right] = [...ids].sort();
  const from = graph.getNodeAttributes(left);
  const to = graph.getNodeAttributes(right);
  const distance = Math.max(SPACING, Math.hypot(from.x - to.x, from.y - to.y));
  graph.setNodeAttribute(left, "x", -distance / 2);
  graph.setNodeAttribute(left, "y", 0);
  graph.setNodeAttribute(right, "x", distance / 2);
  graph.setNodeAttribute(right, "y", 0);
}

function pack(groups: Array<Array<{ id: string; x: number; y: number }>>): Map<string, { x: number; y: number }> {
  const boxes = groups.map((nodes) => {
    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;
    for (const node of nodes) {
      minX = Math.min(minX, node.x);
      minY = Math.min(minY, node.y);
      maxX = Math.max(maxX, node.x);
      maxY = Math.max(maxY, node.y);
    }
    return {
      nodes,
      minX,
      minY,
      width: Math.max(maxX - minX, NODE_SIZE * 2),
      height: Math.max(maxY - minY, NODE_SIZE * 2),
      key: nodes[0]?.id ?? "",
    };
  });
  boxes.sort((left, right) => right.nodes.length - left.nodes.length || left.key.localeCompare(right.key));
  let area = 0;
  let widest = 1;
  for (const box of boxes) {
    area += (box.width + COMPONENT_GAP) * (box.height + COMPONENT_GAP);
    widest = Math.max(widest, box.width);
  }
  const rowWidth = Math.max(widest, Math.sqrt(area));
  let cursorX = 0;
  let cursorY = 0;
  let rowHeight = 0;
  const placed = new Map<string, { x: number; y: number }>();
  for (const box of boxes) {
    if (cursorX > 0 && cursorX + box.width > rowWidth) {
      cursorY += rowHeight + COMPONENT_GAP;
      cursorX = 0;
      rowHeight = 0;
    }
    for (const node of box.nodes) {
      placed.set(node.id, { x: node.x - box.minX + cursorX, y: node.y - box.minY + cursorY });
    }
    cursorX += box.width + COMPONENT_GAP;
    rowHeight = Math.max(rowHeight, box.height);
  }
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const point of placed.values()) {
    minX = Math.min(minX, point.x);
    minY = Math.min(minY, point.y);
    maxX = Math.max(maxX, point.x);
    maxY = Math.max(maxY, point.y);
  }
  const centerX = (minX + maxX) / 2;
  const centerY = (minY + maxY) / 2;
  for (const point of placed.values()) {
    point.x -= centerX;
    point.y -= centerY;
  }
  return placed;
}

export function createLayoutSession(nodeIds: string[], edges: LayoutEdge[]): LayoutSession {
  const model = new Graph({ type: "directed" });
  for (const id of nodeIds) {
    if (!model.hasNode(id)) model.addNode(id);
  }
  for (const edge of edges) {
    if (edge.source === edge.target) continue;
    if (!model.hasNode(edge.source) || !model.hasNode(edge.target)) continue;
    if (model.hasDirectedEdge(edge.source, edge.target)) continue;
    model.addDirectedEdge(edge.source, edge.target);
  }
  const seen = new Set<string>();
  const components: ComponentLayout[] = [];
  model.forEachNode((start) => {
    if (seen.has(start)) return;
    const group: string[] = [];
    const stack = [start];
    seen.add(start);
    while (stack.length > 0) {
      const current = stack.pop();
      if (!current) continue;
      group.push(current);
      for (const neighbor of neighborIds(model, current)) {
        if (seen.has(neighbor)) continue;
        seen.add(neighbor);
        stack.push(neighbor);
      }
    }
    const ids = orderComponent(model, group);
    const packed = openPath(model, ids);
    const local = new Graph({ type: "directed" });
    ids.forEach((id, index) => {
      const point = packed ? placePackedPath(index, ids.length) : placeOnArc(index, ids.length);
      local.addNode(id, { ...point, size: NODE_SIZE });
    });
    const members = new Set(ids);
    model.forEachDirectedEdge((_edge, _attributes, source, target) => {
      if (members.has(source) && members.has(target) && !local.hasDirectedEdge(source, target)) {
        local.addDirectedEdge(source, target);
      }
    });
    components.push({
      graph: local,
      ids,
      remaining: packed ? 0 : iterationBudget(ids.length),
    });
  });
  let cursor = 0;
  let elapsed = 0;
  return {
    step() {
      if (cursor >= components.length) return true;
      const component = components[cursor];
      if (component.remaining <= 0) {
        cursor += 1;
        return cursor >= components.length;
      }
      const batch = component.graph.order > 100 ? Math.min(8, component.remaining) : component.remaining;
      const started = performance.now();
      forceAtlas2.assign(component.graph, { iterations: batch, settings: SETTINGS });
      elapsed += performance.now() - started;
      component.remaining -= batch;
      if (component.remaining <= 0) cursor += 1;
      return cursor >= components.length;
    },
    positions() {
      for (const component of components) levelPair(component.graph, component.ids);
      return pack(
        components.map((component) =>
          component.ids.map((id) => ({
            id,
            x: component.graph.getNodeAttribute(id, "x") as number,
            y: component.graph.getNodeAttribute(id, "y") as number,
          })),
        ),
      );
    },
    milliseconds() {
      return elapsed;
    },
  };
}

/** Runs the whole layout. Tests and small graphs use this; the canvas slices `step`. */
export function layoutGraph(nodeIds: string[], edges: LayoutEdge[]) {
  const session = createLayoutSession(nodeIds, edges);
  while (!session.step()) {
    // Each step bounds the work. A synchronous caller drains the queue.
  }
  return { positions: session.positions(), milliseconds: session.milliseconds() };
}
