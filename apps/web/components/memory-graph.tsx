"use client";

import { useEffect, useRef } from "react";
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

function cssColor(variable: string, fallback: string): string {
  const probe = document.createElement("span");
  probe.style.color = `var(${variable})`;
  document.body.append(probe);
  const color = getComputedStyle(probe).color;
  probe.remove();
  const match = color.match(/rgba?\((\d+),\s*(\d+),\s*(\d+)/);
  if (!match) return fallback;
  const channel = (value: string) => Number(value).toString(16).padStart(2, "0");
  return `#${channel(match[1])}${channel(match[2])}${channel(match[3])}`;
}

function cssBackground(variable: string): string {
  const probe = document.createElement("span");
  probe.style.backgroundColor = `var(${variable})`;
  document.body.append(probe);
  const color = getComputedStyle(probe).backgroundColor;
  probe.remove();
  return color;
}

export function MemoryGraphCanvas({
  graph,
  onSelect,
}: {
  graph: MemoryGraph;
  onSelect: (memoryId: string) => void;
}) {
  const container = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const element = container.current;
    if (!element || graph.nodes.length === 0) return;
    let renderer: { kill: () => void } | null = null;
    let cancelled = false;
    const dragged = { current: false };
    void (async () => {
      const [{ default: Graph }, sigmaModule, { default: forceAtlas2 }, rendering, primitives] = await Promise.all([
        import("graphology"),
        import("sigma"),
        import("graphology-layout-forceatlas2"),
        import("sigma/rendering"),
        import("sigma/primitives"),
      ]);
      if (cancelled || !element.isConnected) return;
      const model = new Graph({ type: "directed", multi: true });
      graph.nodes.forEach((node, index) => {
        const angle = (2 * Math.PI * index) / graph.nodes.length;
        model.addNode(node.memory_id, {
          x: Math.cos(angle),
          y: Math.sin(angle),
          size: 12,
          label: node.title,
          color: cssColor(SCOPE_COLOR[node.scope][0], SCOPE_COLOR[node.scope][1]),
        });
      });
      for (const edge of graph.edges) {
        if (!model.hasNode(edge.source_id) || !model.hasNode(edge.target_id)) continue;
        const style = EDGE_STYLE[edge.relation] ?? EDGE_STYLE.relates;
        model.addEdgeWithKey(`${edge.source_id}:${edge.relation}:${edge.target_id}`, edge.source_id, edge.target_id, {
          color: cssColor(style.variable, style.fallback),
          size: style.size,
          path: style.path,
          head: style.head,
        });
      }
      if (model.order > 1) {
        const inferred = forceAtlas2.inferSettings(model);
        forceAtlas2.assign(model, {
          iterations: model.order > 200 ? 40 : 120,
          settings: { ...inferred, scalingRatio: (inferred.scalingRatio ?? 1) * 8, gravity: 0.4 },
        });
      }
      const Sigma = sigmaModule.default;
      const defaults = primitives.DEFAULT_PRIMITIVES;
      const styles = sigmaModule.DEFAULT_STYLES;
      const instance = new Sigma(model, element, {
        settings: {
          enableNodeDrag: true,
          itemSizesReference: "screen",
          stagePadding: 24,
          labelRenderedSizeThreshold: 1,
          labelDensity: 0.5,
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
          nodes: styles.nodes,
          edges: {
            ...styles.edges,
            path: { attribute: "path", defaultValue: "straight" },
            head: { attribute: "head", defaultValue: "none" },
          },
          stage: { background: cssBackground("--card") },
        },
      });
      renderer = instance;
      if (cancelled) {
        instance.kill();
        return;
      }
      instance.on("nodeDragStart", () => {
        dragged.current = true;
      });
      instance.on("clickNode", ({ node }) => {
        if (dragged.current) {
          dragged.current = false;
          return;
        }
        onSelect(node);
      });
    })();
    return () => {
      cancelled = true;
      renderer?.kill();
    };
  }, [graph, onSelect]);
  return (
    <div
      ref={container}
      data-testid="memory-graph"
      role="img"
      aria-label="Memory link graph"
      className="h-[70vh] min-h-80 w-full touch-none rounded-lg border bg-card"
    />
  );
}
