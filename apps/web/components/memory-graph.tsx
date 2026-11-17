"use client";

import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { graphEdgeKey } from "@/lib/graph-edge-key";
import { NODE_SIZE, createLayoutSession, fitBBox } from "@/lib/graph-layout";
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
    let renderer: { kill: () => void } | null = null;
    let timeout = 0;
    let cancelled = false;
    delete element.dataset.layoutMs;
    const fail = (cause: unknown) => {
      if (cancelled) return;
      setFailure({
        graph,
        dark,
        message: cause instanceof Error ? cause.message : "Could not draw the graph",
      });
    };
    void (async () => {
      try {
        const [{ default: Graph }, sigmaModule, rendering, primitives] = await Promise.all([
          import("graphology"),
          import("sigma"),
          import("sigma/rendering"),
          import("sigma/primitives"),
        ]);
        if (cancelled || !element.isConnected) return;
        const model = new Graph({ type: "directed", multi: true });
        for (const node of graph.nodes) {
          model.addNode(node.memory_id, {
            x: 0,
            y: 0,
            size: NODE_SIZE,
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
        const session = createLayoutSession(
          graph.nodes.map((node) => node.memory_id),
          graph.edges.map((edge) => ({ source: edge.source_id, target: edge.target_id })),
        );
        const mount = () => {
          if (cancelled || !element.isConnected) return;
          try {
            if (!session.step()) {
              timeout = window.setTimeout(mount, 0);
              return;
            }
            session.positions().forEach((point, id) => {
              model.setNodeAttribute(id, "x", point.x);
              model.setNodeAttribute(id, "y", point.y);
            });
            const label = cssHex("--foreground", dark ? "#f4f4f5" : "#1c1e22", "color");
            const card = cssHex("--card", dark ? "#16181c" : "#ffffff", "backgroundColor");
            const Sigma = sigmaModule.default;
            const defaults = primitives.DEFAULT_PRIMITIVES;
            const styles = sigmaModule.DEFAULT_STYLES;
            const instance = new Sigma(model, element, {
              settings: {
                enableNodeDrag: true,
                itemSizesReference: "positions",
                autoRescale: true,
                autoRescaleContent: model.order <= 80 ? "labels" : "nodes",
                stagePadding: 48,
                // Quadratic so zooming in enlarges nodes; ratio 1 matches graph units.
                zoomToSizeRatioFunction: (ratio) => ratio * ratio,
                labelRenderedSizeThreshold: 3,
                labelDensity: 1,
                labelGridCellSize: 200,
                nodePickingPadding: 8,
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
            const frame = () => {
              const box = fitBBox(session.positions().values(), element.getBoundingClientRect(), 48);
              instance.setCustomBBox(box);
            };
            frame();
            const resize = new ResizeObserver(frame);
            resize.observe(element);
            renderer = {
              kill: () => {
                resize.disconnect();
                instance.kill();
              },
            };
            if (cancelled) {
              renderer.kill();
              return;
            }
            element.dataset.layoutRun = String(Number(element.dataset.layoutRun ?? "0") + 1);
            element.dataset.layoutMs = String(Math.round(session.milliseconds()));
            element.dataset.nodeCount = String(model.order);
            instance.on("clickNode", ({ node }: { node: string }) => {
              onSelect(node);
            });
          } catch (cause) {
            fail(cause);
          }
        };
        mount();
      } catch (cause) {
        fail(cause);
      }
    })();
    return () => {
      cancelled = true;
      window.clearTimeout(timeout);
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
