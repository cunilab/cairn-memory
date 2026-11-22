"use client";

import { useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { WaypointsIcon } from "lucide-react";
import { FilterSelect } from "@/components/filter-select";
import { ErrorAlert, LoadingBlock } from "@/components/states";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { useIsMobile } from "@/hooks/use-mobile";
import { useOpenMemory, useQueryParams } from "@/hooks/use-query-params";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { cairn } from "@/lib/cairn";
import { LOGIN_PATH, SessionRequiredError } from "@/lib/session";
import type { Graph } from "@/lib/generated/Graph";
import type { ProjectView } from "@/lib/generated/ProjectView";
import { LinkTable } from "./link-table";
import { MemoryGraphCanvas } from "./memory-graph";

const RELATIONS = ["relates", "causes", "fixes", "depends_on", "contradicts", "supersedes"].map((value) => ({
  value,
  label: value.replaceAll("_", " "),
}));
const SCOPES = ["personal", "project", "global"].map((value) => ({ value, label: value.charAt(0).toUpperCase() + value.slice(1) }));
const KINDS = ["decision", "rule", "gotcha", "procedure", "fact", "preference", "note", "handoff"].map((value) => ({
  value,
  label: value.charAt(0).toUpperCase() + value.slice(1),
}));

type Loaded = { key: string; graph: Graph | null; projects: ProjectView[]; error: string | null };

export function GraphView() {
  const router = useRouter();
  const mobile = useIsMobile();
  const openMemory = useOpenMemory();
  const [params, set] = useQueryParams();
  // project and kind live in the URL, shared with Memories; the rest only shape the graph query.
  const project = params.get("project") ?? "";
  const kind = params.get("kind") ?? "";
  const [scope, setScope] = useState("");
  const [relation, setRelation] = useState("");
  const [cap, setCap] = useState(120);
  const [capText, setCapText] = useState("120");
  const [attempt, setAttempt] = useState(0);
  const [loaded, setLoaded] = useState<Loaded | null>(null);

  const query = useMemo(() => {
    const next = new URLSearchParams({ cap: String(cap) });
    if (scope) next.set("scope", scope);
    if (project) next.set("projectId", project);
    if (relation) next.set("relation", relation);
    return next.toString();
  }, [cap, scope, project, relation]);
  const key = `${query}#${attempt}`;

  useResetOnHide(() => setLoaded(null));

  useEffect(() => {
    let cancelled = false;
    Promise.all([cairn<Graph>(`graph?${query}`), cairn<ProjectView[]>("projects")])
      .then(([graph, projects]) => !cancelled && setLoaded({ key, graph, projects, error: null }))
      .catch((cause: unknown) => {
        if (cancelled) return;
        if (cause instanceof SessionRequiredError) {
          router.replace(LOGIN_PATH);
          return;
        }
        setLoaded({
          key,
          graph: null,
          projects: [],
          error: `The graph could not be loaded. ${cause instanceof Error ? cause.message : "Check that the server is running, then retry."}`,
        });
      });
    return () => {
      cancelled = true;
    };
  }, [key, query, router]);

  // Graph nodes carry no status, so `status` is not a graph filter; `kind` is applied here.
  const graph = useMemo<Graph | null>(() => {
    const full = loaded?.graph;
    if (!full || !kind) return full ?? null;
    const nodes = full.nodes.filter((node) => node.kind === kind);
    const ids = new Set(nodes.map((node) => node.memory_id));
    return { ...full, nodes, edges: full.edges.filter((e) => ids.has(e.source_id) && ids.has(e.target_id)) };
  }, [loaded, kind]);

  const listHref = `/memories?${new URLSearchParams(Object.entries({ project, kind }).filter(([, v]) => v))}`;
  const current = loaded?.key === key ? loaded : null;
  const projects = loaded?.projects ?? [];

  return (
    <div className="flex min-w-0 flex-col gap-4">
      <div className="grid grid-cols-2 gap-2 md:grid-cols-5">
        <FilterSelect label="Project" value={project} allLabel="All projects" options={projects.map((p) => ({ value: p.project_id, label: p.name }))} onChange={(v) => set({ project: v })} />
        <FilterSelect label="Kind" value={kind} allLabel="All kinds" options={KINDS} onChange={(v) => set({ kind: v })} />
        <FilterSelect label="Scope" value={scope} allLabel="All scopes" options={SCOPES} onChange={setScope} />
        <FilterSelect label="Relation" value={relation} allLabel="All relations" options={RELATIONS} onChange={setRelation} />
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
        <li><span aria-hidden className="mr-1 inline-block size-2 rounded-full bg-chart-2" />Personal</li>
        <li><span aria-hidden className="mr-1 inline-block size-2 rounded-full bg-primary" />Project</li>
        <li><span aria-hidden className="mr-1 inline-block size-2 rounded-full bg-foreground" />Global</li>
        <li>Plain line: relates. Arrows: causes, fixes, depends on, contradicts, supersedes.</li>
      </ul>
      <p className="text-sm">
        <Link href={listHref} className="font-medium text-brand underline-offset-4 hover:underline">
          List view in Memories
        </Link>
      </p>
      {current?.error ? <ErrorAlert message={current.error} onRetry={() => { setLoaded(null); setAttempt((n) => n + 1); }} /> : null}
      {current === null ? <LoadingBlock label="Loading the graph…" rows={4} /> : null}
      {graph?.truncated ? (
        <p className="text-sm text-muted-foreground">
          Showing {graph.nodes.length} memories. The least-connected, oldest ones were left out. Raise the cap or narrow the filters.
        </p>
      ) : null}
      {current?.graph && graph ? (
        graph.edges.length === 0 ? (
          <Empty className="border">
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <WaypointsIcon />
              </EmptyMedia>
              <EmptyTitle>No links yet</EmptyTitle>
              <EmptyDescription>Cairn links memories that share files and symbols automatically.</EmptyDescription>
            </EmptyHeader>
          </Empty>
        ) : mobile ? (
          <p className="rounded-lg border p-4 text-sm text-muted-foreground">The graph needs a wider screen. Use the list view instead.</p>
        ) : (
          <>
            <MemoryGraphCanvas graph={graph} onSelect={openMemory} />
            <LinkTable graph={graph} onOpen={openMemory} />
          </>
        )
      ) : null}
    </div>
  );
}
