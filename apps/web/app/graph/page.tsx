"use client";

import Link from "next/link";
import { useCallback, useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { MemoryGraphCanvas } from "@/components/memory-graph";
import { ConsoleShell } from "@/components/console-shell";
import { PageHeader } from "@/components/page-header";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Input } from "@/components/ui/input";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { cairn } from "@/lib/cairn";
import { SessionRequiredError } from "@/lib/session";
import type { ListedProject, MemoryGraph, ProjectMemory } from "@/lib/types";

const RELATIONS = ["relates", "causes", "fixes", "depends_on", "contradicts", "supersedes"] as const;
const SCOPES = ["personal", "project", "global"] as const;

const controlClass =
  "h-8 w-full rounded-lg border border-input bg-transparent px-2.5 text-sm outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 dark:bg-input/30";

export default function GraphPage() {
  const router = useRouter();
  const [graph, setGraph] = useState<MemoryGraph | null>(null);
  const [projects, setProjects] = useState<ListedProject[]>([]);
  const [scope, setScope] = useState("");
  const [projectId, setProjectId] = useState("");
  const [relation, setRelation] = useState("");
  const [cap, setCap] = useState(120);
  const [capText, setCapText] = useState("120");
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [detail, setDetail] = useState<ProjectMemory | null>(null);
  const openMemory = useCallback((memoryId: string) => {
    cairn<ProjectMemory>(`memories/${memoryId}`)
      .then(setDetail)
      .catch((cause: unknown) => {
        setError(cause instanceof Error ? cause.message : "Could not open that memory");
      });
  }, []);
  useResetOnHide(() => {
    setGraph(null);
    setProjects([]);
    setDetail(null);
    setError(null);
  });
  useEffect(() => {
    let cancelled = false;
    const params = new URLSearchParams({ cap: String(cap) });
    if (scope) params.set("scope", scope);
    if (projectId) params.set("projectId", projectId);
    if (relation) params.set("relation", relation);
    Promise.all([cairn<MemoryGraph>(`graph?${params}`), cairn<ListedProject[]>("projects")])
      .then(([nextGraph, nextProjects]) => {
        if (!cancelled) {
          setGraph(nextGraph);
          setProjects(nextProjects);
          setError(null);
        }
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        if (cause instanceof SessionRequiredError) router.replace("/");
        else setError(cause instanceof Error ? cause.message : "Could not load the graph");
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [cap, projectId, relation, router, scope]);
  return (
    <ConsoleShell crumbs={[{ label: "Graph" }]}>
      <PageHeader
        title="Graph"
        description="Every memory you can read, and the links between them."
      />
      {error ? (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}
      <div className="grid grid-cols-2 gap-2 md:grid-cols-4">
        <label className="grid gap-1 text-xs font-medium text-muted-foreground">
          Scope
          <select className={controlClass} value={scope} onChange={(event) => setScope(event.target.value)} aria-label="Scope">
            <option value="">All</option>
            {SCOPES.map((item) => (
              <option key={item} value={item}>{item}</option>
            ))}
          </select>
        </label>
        <label className="grid gap-1 text-xs font-medium text-muted-foreground">
          Project
          <select className={controlClass} value={projectId} onChange={(event) => setProjectId(event.target.value)} aria-label="Project">
            <option value="">All</option>
            {projects.map((project) => (
              <option key={project.project_id} value={project.project_id}>{project.name}</option>
            ))}
          </select>
        </label>
        <label className="grid gap-1 text-xs font-medium text-muted-foreground">
          Relation
          <select className={controlClass} value={relation} onChange={(event) => setRelation(event.target.value)} aria-label="Relation">
            <option value="">All</option>
            {RELATIONS.map((item) => (
              <option key={item} value={item}>{item.replaceAll("_", " ")}</option>
            ))}
          </select>
        </label>
        <label className="grid gap-1 text-xs font-medium text-muted-foreground">
          Node cap
          <Input
            type="number"
            min={120}
            max={500}
            value={capText}
            aria-label="Node cap"
            onChange={(event) => {
              setCapText(event.target.value);
              const next = Number(event.target.value);
              if (Number.isInteger(next) && next >= 120 && next <= 500) setCap(next);
            }}
          />
        </label>
      </div>
      <ul className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
        <li><span className="mr-1 inline-block size-2 rounded-full bg-chart-2" />Personal</li>
        <li><span className="mr-1 inline-block size-2 rounded-full bg-primary" />Project</li>
        <li><span className="mr-1 inline-block size-2 rounded-full bg-foreground" />Global</li>
        <li>Plain line relates. Arrows are causes, fixes, depends on, contradicts, and supersedes.</li>
      </ul>
      {graph?.truncated ? (
        <p className="text-sm text-muted-foreground">
          Showing {graph.nodes.length} memories. The least-connected, oldest ones were left out. Raise the cap or narrow the filters.
        </p>
      ) : null}
      {graph && graph.nodes.length > 0 ? (
        <MemoryGraphCanvas graph={graph} onSelect={openMemory} />
      ) : (
        <p className="rounded-lg border p-6 text-sm text-muted-foreground">
          {loading ? "Loading the graph…" : "No memories to draw."}
        </p>
      )}
      <details className="text-sm">
        <summary className="cursor-pointer font-medium">Memory list</summary>
        <ul className="mt-2 grid gap-1">
          {graph?.nodes.map((node) => (
            <li key={node.memory_id}>
              <button type="button" className="text-left hover:underline" onClick={() => openMemory(node.memory_id)}>
                {node.title}
              </button>
              <span className="ml-2 text-muted-foreground">{node.scope}</span>
            </li>
          ))}
        </ul>
      </details>
      <Sheet open={detail !== null} onOpenChange={(open) => !open && setDetail(null)}>
        <SheetContent className="overflow-y-auto">
          <SheetHeader>
            <SheetTitle>{detail?.title ?? "Memory"}</SheetTitle>
            <SheetDescription>{detail?.scope ?? "Memory"}</SheetDescription>
          </SheetHeader>
          <div className="space-y-4 p-4">
            <Link href="/memories" className="text-sm font-medium text-brand underline-offset-4 hover:underline">
              Open memories
            </Link>
            {detail
              ? Object.entries(detail).map(([field, value]) => (
                  <div key={field}>
                    <p className="text-xs font-medium text-muted-foreground">{field.replaceAll("_", " ")}</p>
                    <p className="whitespace-pre-wrap break-words text-sm">
                      {typeof value === "object" && value !== null ? JSON.stringify(value, null, 2) : String(value ?? "—")}
                    </p>
                  </div>
                ))
              : null}
          </div>
        </SheetContent>
      </Sheet>
    </ConsoleShell>
  );
}
