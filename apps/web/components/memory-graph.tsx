"use client";

import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import type Graph from "graphology";
import type forceAtlas2 from "graphology-layout-forceatlas2";
import { graphEdgeKey } from "@/lib/graph-edge-key";
import type { MemoryGraph } from "@/lib/types";

const SCOPE_COLOR = {
  personal: ["--chart-2", "#5c6370"],
  project: ["--primary", "#e0a100"],
  global: ["--foreground", "#1c1e22"],
} as const;

const EDGE_STYLE: Record<string, { variable: string; fallback: string; size: number; path: string; head: string }> = {
  relates: { variable: "--muted-foreground", fallback: "#8b909a", size: 1.5, path: "straight", head: "none" },
  causes: { variable: "--primary", fallback: "#e0a100", size: 2.5, path: "straight", head: "arrow" },
  fixes: { variable: "--success", fallback: "#2f7d4a", size: 2.5, path: "straight", head: "arrow" },
  depends_on: { variable: "--chart-3", fallback: "#6b7280", size: 2, path: "curved", head: "arrow" },
  contradicts: { variable: "--destructive", fallback: "#b42318", size: 2.5, path: "straight", head: "arrow" },
  supersedes: { variable: "--foreground", fallback: "#1c1e22", size: 3, path: "curved", head: "arrow" },
};

function subscribeDarkClass(onChange: () => void) {
  const observer = new MutationObserver(onChange);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
  return () => observer.disconnect();
}

function canvasLabel(title: string): string {
  const limit = 22;
  return title.length > limit ? `${title.slice(0, limit - 1)}…` : title;
}

function rasterHex(color: string, fallback: string): string {
  const canvas = document.createElement("canvas");
  canvas.width = 1;
  canvas.height = 1;
  const context = canvas.getContext("2d", { willReadFrequently: true });
  if (!context) return fallback;
  context.fillStyle = "#010203";
  context.fillStyle = color;
  if (context.fillStyle === "#010203") return fallback;
  context.fillRect(0, 0, 1, 1);
  const [red, green, blue, alpha] = context.getImageData(0, 0, 1, 1).data;
  if (alpha === 0) return fallback;
  const channel = (value: number) => value.toString(16).padStart(2, "0");
  return `#${channel(red)}${channel(green)}${channel(blue)}`;
}

function cssHex(variable: string, fallback: string, paint: "color" | "backgroundColor"): string {
  const probe = document.createElement("span");
  probe.style[paint] = `var(${variable})`;
  document.body.append(probe);
  const color = getComputedStyle(probe)[paint];
  probe.remove();
  return rasterHex(color, fallback);
}

type LaidOut = { id: string; x: number; y: number };

function weakComponents(model: Graph): string[][] {
  const seen = new Set<string>();
  const groups: string[][] = [];
  model.forEachNode((start) => {
    if (seen.has(start)) return;
    const group: string[] = [];
    const stack = [start];
    seen.add(start);
    while (stack.length > 0) {
      const current = stack.pop();
      if (!current) continue;
      group.push(current);
      model.forEachNeighbor(current, (neighbor) => {
        if (seen.has(neighbor)) return;
        seen.add(neighbor);
        stack.push(neighbor);
      });
    }
    groups.push(group);
  });
  return groups;
}

function layoutComponent(
  source: Graph,
  GraphCtor: new (options: { type: "directed"; multi: true }) => Graph,
  assign: typeof forceAtlas2.assign,
  ids: string[],
): LaidOut[] {
  if (ids.length === 1) return [{ id: ids[0], x: 0, y: 0 }];
  if (ids.length === 2) {
    return [
      { id: ids[0], x: -90, y: 0 },
      { id: ids[1], x: 90, y: 0 },
    ];
  }
  const local = new GraphCtor({ type: "directed", multi: true });
  const members = new Set(ids);
  ids.forEach((id, index) => {
    const angle = (2 * Math.PI * index) / ids.length;
    local.addNode(id, { x: Math.cos(angle) * 10, y: Math.sin(angle) * 10 });
  });
  source.forEachEdge((_edge, _attributes, from, to) => {
    if (members.has(from) && members.has(to)) local.addEdge(from, to);
  });
  assign(local, {
    iterations: ids.length > 200 ? 50 : 80,
    settings: {
      gravity: 0.08,
      scalingRatio: ids.length > 80 ? 14 : 6,
      strongGravityMode: false,
      barnesHutOptimize: ids.length > 80,
      slowDown: 1,
    },
  });
  return ids.map((id) => ({
    id,
    x: local.getNodeAttribute(id, "x"),
    y: local.getNodeAttribute(id, "y"),
  }));
}

function packComponents(groups: LaidOut[][], targetWidth: number) {
  const boxes = groups.map((nodes) => {
    const xs = nodes.map((node) => node.x);
    const ys = nodes.map((node) => node.y);
    const minX = Math.min(...xs);
    const minY = Math.min(...ys);
    return {
      nodes,
      minX,
      minY,
      width: Math.max(...xs) - minX || 1,
      height: Math.max(...ys) - minY || 1,
    };
  });
  const gap = 140;
  const rowWidth = Math.max(targetWidth, 1);
  let cursorX = 0;
  let cursorY = 0;
  let rowHeight = 0;
  const placed = new Map<string, { x: number; y: number }>();
  for (const box of boxes) {
    if (cursorX > 0 && cursorX + box.width > rowWidth) {
      cursorY += rowHeight + gap;
      cursorX = 0;
      rowHeight = 0;
    }
    for (const node of box.nodes) {
      placed.set(node.id, { x: node.x - box.minX + cursorX, y: node.y - box.minY + cursorY });
    }
    cursorX += box.width + gap;
    rowHeight = Math.max(rowHeight, box.height);
  }
  let sumX = 0;
  let sumY = 0;
  for (const point of placed.values()) {
    sumX += point.x;
    sumY += point.y;
  }
  const centerX = sumX / placed.size;
  const centerY = sumY / placed.size;
  return { placed, centerX, centerY };
}

export function MemoryGraphCanvas({
  graph,
  onSelect,
}: {
  graph: MemoryGraph;
  onSelect: (memoryId: string) => void;
}) {
  const container = useRef<HTMLDivElement>(null);
  const [failure, setFailure] = useState<{ graph: MemoryGraph; dark: boolean; message: string } | null>(null);
  const dark = useSyncExternalStore(subscribeDarkClass, () => document.documentElement.classList.contains("dark"), () => false);
  useEffect(() => {
    const element = container.current;
    if (!element || graph.nodes.length === 0) return;
    let renderer: { kill: () => void; getCamera: () => { ratio: number; setState: (state: { ratio: number }) => void } } | null = null;
    let cancelled = false;
    void (async () => {
      try {
        const [{ default: Graph }, sigmaModule, { default: forceAtlas2 }, rendering, primitives] = await Promise.all([
          import("graphology"),
          import("sigma"),
          import("graphology-layout-forceatlas2"),
          import("sigma/rendering"),
          import("sigma/primitives"),
        ]);
        if (cancelled || !element.isConnected) return;
        const model = new Graph({ type: "directed", multi: true });
        for (const node of graph.nodes) {
          model.addNode(node.memory_id, {
            x: 0,
            y: 0,
            size: 12,
            label: canvasLabel(node.title),
            color: cssHex(SCOPE_COLOR[node.scope][0], SCOPE_COLOR[node.scope][1], "color"),
          });
        }
        for (const edge of graph.edges) {
          if (!model.hasNode(edge.source_id) || !model.hasNode(edge.target_id)) continue;
          const key = graphEdgeKey(edge.source_id, edge.relation, edge.target_id);
          if (model.hasEdge(key)) continue;
          const style = EDGE_STYLE[edge.relation] ?? EDGE_STYLE.relates;
          model.addEdgeWithKey(key, edge.source_id, edge.target_id, {
            color: cssHex(style.variable, style.fallback, "color"),
            size: style.size,
            path: style.path,
            head: style.head,
          });
        }
        if (model.order > 1) {
          const groups = weakComponents(model);
          const laid = groups.map((ids) => layoutComponent(model, Graph, forceAtlas2.assign, ids));
          const width = element.clientWidth;
          const height = element.clientHeight;
          const { placed, centerX, centerY } = packComponents(laid, Math.max(width - 160, 180));
          let maxX = 1;
          let maxY = 1;
          for (const point of placed.values()) {
            maxX = Math.max(maxX, Math.abs(point.x - centerX));
            maxY = Math.max(maxY, Math.abs(point.y - centerY));
          }
          const pad = 72;
          const fit = Math.min(
            Math.max(width / 2 - pad, 48) / maxX,
            Math.max(height / 2 - pad, 48) / maxY,
          );
          const sparse = groups.length > 1 && model.order <= 24;
          const scale = sparse ? Math.min(1, fit) : fit;
          placed.forEach((point, id) => {
            model.setNodeAttribute(id, "x", (point.x - centerX) * scale);
            model.setNodeAttribute(id, "y", (point.y - centerY) * scale);
          });
        }
        const label = cssHex("--foreground", dark ? "#f4f4f5" : "#1c1e22", "color");
        const card = cssHex("--card", dark ? "#16181c" : "#ffffff", "backgroundColor");
        const Sigma = sigmaModule.default;
        const defaults = primitives.DEFAULT_PRIMITIVES;
        const styles = sigmaModule.DEFAULT_STYLES;
        const instance = new Sigma(model, element, {
          settings: {
            enableNodeDrag: true,
            itemSizesReference: "screen",
            autoRescale: false,
            stagePadding: 48,
            labelRenderedSizeThreshold: 1,
            labelDensity: 0.8,
            labelGridCellSize: 190,
          },
          primitives: {
            ...defaults,
            edges: {
              ...defaults.edges,
              paths: [...(defaults.edges.paths ?? []), rendering.pathCurved()],
              extremities: [...(defaults.edges.extremities ?? []), rendering.extremityArrow()],
            },
          },
          styles: {
            nodes: {
              ...styles.nodes,
              labelColor: label,
              labelPosition: "above",
              labelBackgroundColor: card,
              labelBackgroundPadding: 3,
            },
            edges: {
              ...styles.edges,
              path: { attribute: "path", defaultValue: "straight" },
              head: { attribute: "head", defaultValue: "none" },
            },
            stage: { background: card },
          },
        });
        renderer = instance;
        if (cancelled) {
          instance.kill();
          return;
        }
        instance.on("clickNode", ({ node }: { node: string }) => {
          onSelect(node);
        });
      } catch (cause) {
        if (!cancelled) {
          setFailure({
            graph,
            dark,
            message: cause instanceof Error ? cause.message : "Could not draw the graph",
          });
        }
      }
    })();
    return () => {
      cancelled = true;
      renderer?.kill();
    };
  }, [dark, graph, onSelect]);
  const message = failure && failure.graph === graph && failure.dark === dark ? failure.message : null;
  return (
    <div className="grid gap-2">
      {message ? (
        <p role="alert" className="text-sm text-destructive">
          {message}
        </p>
      ) : null}
      <div
        ref={container}
        data-testid="memory-graph"
        role="img"
        aria-label="Memory link graph"
        aria-describedby="memory-links"
        className="h-[70vh] min-h-80 w-full touch-none rounded-lg border bg-card"
      />
    </div>
  );
}
