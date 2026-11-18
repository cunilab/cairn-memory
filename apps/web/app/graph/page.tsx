"use client";

import Link from "next/link";
import { useCallback, useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { MemoryGraphCanvas } from "@/components/memory-graph";
import { ConsoleShell } from "@/components/console-shell";
import { PageHeader } from "@/components/page-header";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Table, TableBody, TableCaption, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { graphEdgeKey } from "@/lib/graph-edge-key";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { cairn } from "@/lib/cairn";
import { SessionRequiredError } from "@/lib/session";
import type { ListedProject, MemoryGraph, ProjectMemory } from "@/lib/types";

const RELATIONS = ["relates", "causes", "fixes", "depends_on", "contradicts", "supersedes"] as const;
const SCOPES = ["personal", "project", "global"] as const;

function choice(value: string | null) {
  return value && value !== "all" ? value : "";
}

function LinkTable({ graph, onOpen }: { graph: MemoryGraph; onOpen: (memoryId: string) => void }) {
  const titles = new Map(graph.nodes.map((node) => [node.memory_id, node.title]));
  const linked = new Set(graph.edges.flatMap((edge) => [edge.source_id, edge.target_id]));
  const alone = graph.nodes.filter((node) => !linked.has(node.memory_id));
  return (
    <div className="max-h-80 overflow-auto rounded-lg border">
      <Table id="memory-links" aria-label="Memory links">
        <TableCaption className="sr-only">
          Directed links. Each row names the source memory, the relation, and the target memory.
        </TableCaption>
        <TableHeader>
          <TableRow>
            <TableHead>Source</TableHead>
            <TableHead>Relation</TableHead>
            <TableHead>Target</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {graph.edges.map((edge) => (
            <TableRow key={graphEdgeKey(edge.source_id, edge.relation, edge.target_id)}>
              <TableCell className="whitespace-normal">
                <button type="button" className="text-left hover:underline" onClick={() => onOpen(edge.source_id)}>
                  {titles.get(edge.source_id) ?? edge.source_id}
                </button>
              </TableCell>
              <TableCell>{edge.relation.replaceAll("_", " ")}</TableCell>
              <TableCell className="whitespace-normal">
                <button type="button" className="text-left hover:underline" onClick={() => onOpen(edge.target_id)}>
                  {titles.get(edge.target_id) ?? edge.target_id}
                </button>
              </TableCell>
            </TableRow>
          ))}
          {alone.map((node) => (
            <TableRow key={`node:${node.memory_id}`}>
              <TableCell className="whitespace-normal">
                <button type="button" className="text-left hover:underline" onClick={() => onOpen(node.memory_id)}>
                  {node.title}
                </button>
              </TableCell>
              <TableCell>no link</TableCell>
              <TableCell>—</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}

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
          <Select value={scope || "all"} onValueChange={(value) => setScope(choice(value))}>
            <SelectTrigger className="w-full" aria-label="Scope">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All</SelectItem>
              {SCOPES.map((item) => (
                <SelectItem key={item} value={item}>{item}</SelectItem>
              ))}
            </SelectContent>
          </Select>
        </label>
        <label className="grid gap-1 text-xs font-medium text-muted-foreground">
          Project
          <Select value={projectId || "all"} onValueChange={(value) => setProjectId(choice(value))}>
            <SelectTrigger className="w-full" aria-label="Project">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All</SelectItem>
              {projects.map((project) => (
                <SelectItem key={project.project_id} value={project.project_id}>{project.name}</SelectItem>
              ))}
            </SelectContent>
          </Select>
        </label>
        <label className="grid gap-1 text-xs font-medium text-muted-foreground">
          Relation
          <Select value={relation || "all"} onValueChange={(value) => setRelation(choice(value))}>
            <SelectTrigger className="w-full" aria-label="Relation">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">All</SelectItem>
              {RELATIONS.map((item) => (
                <SelectItem key={item} value={item}>{item.replaceAll("_", " ")}</SelectItem>
              ))}
            </SelectContent>
          </Select>
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
      {graph && graph.nodes.length > 0 ? (
        <LinkTable graph={graph} onOpen={openMemory} />
      ) : null}
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
