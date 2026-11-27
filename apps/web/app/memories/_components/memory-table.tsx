"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import { Pin, BrainIcon, FilterIcon, SearchIcon, XIcon } from "lucide-react";
import { KindBadge, StatusBadge } from "@/components/badges";
import { FilterSelect, type FilterOption } from "@/components/filter-select";
import { PageNav } from "@/components/page-nav";
import { PinSummaryBar } from "@/components/pin-summary";
import { ErrorAlert, LoadingBlock } from "@/components/states";
import { Time } from "@/components/time";
import { Button } from "@/components/ui/button";
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useIsMobile } from "@/hooks/use-mobile";
import { useQueryParams } from "@/hooks/use-query-params";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { ApiError, cairn } from "@/lib/cairn";
import { canTogglePin, quotaFor, type ConsoleMatch, type ConsoleMemory, type ConsolePage, type PinQuota } from "@/lib/console";
import { useAccount } from "@/hooks/use-account";
import { onMemoriesChanged, announceMemoriesChanged } from "@/lib/memory-events";
import { loginHref, SessionRequiredError } from "@/lib/session";
import type { BranchSummary } from "@/lib/generated/BranchSummary";
import type { ProjectView } from "@/lib/generated/ProjectView";
import type { TrustLevel } from "@/lib/generated/TrustLevel";
import { cn } from "@/lib/utils";

const KINDS = ["decision", "rule", "gotcha", "procedure", "fact", "preference", "note", "handoff"];
const STATUSES: FilterOption[] = [
  { value: "active", label: "Active" },
  { value: "stale", label: "Stale" },
  { value: "superseded", label: "Superseded" },
];
const KIND_OPTIONS: FilterOption[] = KINDS.map((kind) => ({
  value: kind,
  label: kind.charAt(0).toUpperCase() + kind.slice(1),
}));
const TRUST_LABELS = { proven: "Proven", normal: "Normal", new: "New", doubted: "Doubted" } satisfies Record<TrustLevel, string>;
const TRUST_OPTIONS: FilterOption[] = Object.entries(TRUST_LABELS).map(([value, label]) => ({ value, label }));
const PAGE_SIZES = new Set(["25", "50", "100"]);

function MatchExcerpt({ match }: { match: ConsoleMatch }) {
  return (
    <p className="line-clamp-2 text-sm text-muted-foreground">
      <span className="font-medium text-foreground">{match.label}</span>{" "}
      {match.segments.map((segment, index) =>
        segment.hit ? (
          <mark key={index} className="rounded-sm bg-primary/35 px-0.5 text-foreground">
            {segment.text}
          </mark>
        ) : (
          <span key={index}>{segment.text}</span>
        ),
      )}
    </p>
  );
}

export function MemoryTable({ projectId }: { projectId?: string }) {
  const router = useRouter();
  const mobile = useIsMobile();
  const account = useAccount();
  const searchRef = useRef<HTMLInputElement>(null);
  const statuses: FilterOption[] = account?.is_admin
    ? [...STATUSES, { value: "forgotten", label: "Forgotten" }]
    : STATUSES;
  const [params, set] = useQueryParams();
  const project = projectId ?? params.get("project") ?? "";
  const kind = params.get("kind") ?? "";
  const status = params.get("status") ?? "";
  const trust = params.get("trust") ?? "";
  const branch = params.get("branch") ?? "";
  const q = params.get("q") ?? "";
  const match = params.get("match") === "related" ? "related" : "exact";
  const view = params.get("view") === "pinned" ? "pinned" : "all";
  const page = Math.max(1, Number(params.get("page")) || 1);
  const pageSize = PAGE_SIZES.has(params.get("pageSize") ?? "") ? Number(params.get("pageSize")) : 25;
  const focusSearch = params.get("focus") === "search";
  const [search, setSearch] = useState(q);
  const [projects, setProjects] = useState<ProjectView[]>([]);
  const [branches, setBranches] = useState<{ project: string; rows: BranchSummary[] }>({ project: "", rows: [] });
  const [data, setData] = useState<ConsolePage | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loadedKey, setLoadedKey] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [pinning, setPinning] = useState<string | null>(null);

  const serverQuery = useMemo(() => {
    const query = new URLSearchParams({
      page: String(page),
      pageSize: String(pageSize),
      sort: "updated_desc",
      match,
    });
    if (project) query.set("projectId", project);
    if (projectId) query.set("scope", "project");
    if (kind) query.set("kind", kind);
    if (status) query.set("status", status);
    if (trust) query.set("trust", trust);
    if (branch) query.set("branch", branch);
    if (q.trim()) query.set("q", q.trim());
    if (view === "pinned") query.set("pinned", "true");
    return query.toString();
  }, [project, projectId, kind, status, trust, branch, q, match, view, page, pageSize]);

  const requireSignIn = useCallback(() => router.replace(loginHref()), [router]);
  useResetOnHide(() => {
    setData(null);
    setProjects([]);
    setBranches({ project: "", rows: [] });
  });

  useEffect(() => {
    if (focusSearch) searchRef.current?.focus();
  }, [focusSearch]);

  useEffect(() => {
    if (projectId) return;
    let cancelled = false;
    cairn<ProjectView[]>("projects")
      .then((rows) => !cancelled && setProjects(rows))
      .catch((cause: unknown) => {
        if (!cancelled && cause instanceof SessionRequiredError) requireSignIn();
      });
    return () => {
      cancelled = true;
    };
  }, [projectId, requireSignIn]);

  useEffect(() => {
    if (!project) return;
    let cancelled = false;
    cairn<BranchSummary[]>(`projects/${encodeURIComponent(project)}/branches`)
      .then((rows) => !cancelled && setBranches({ project, rows }))
      .catch(() => !cancelled && setBranches({ project, rows: [] }));
    return () => {
      cancelled = true;
    };
  }, [project]);

  useEffect(() => {
    let cancelled = false;
    cairn<ConsolePage>(`console/memories?${serverQuery}`)
      .then((page) => {
        if (cancelled) return;
        setData(page);
        setError(null);
        setLoadedKey(serverQuery);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        if (cause instanceof SessionRequiredError) {
          requireSignIn();
          return;
        }
        setError(`Memories could not be loaded. ${cause instanceof Error ? cause.message : "Check that the server is running, then retry."}`);
        setLoadedKey(serverQuery);
      });
    return () => {
      cancelled = true;
    };
  }, [serverQuery, attempt, requireSignIn]);

  useEffect(() => onMemoriesChanged(() => setAttempt((n) => n + 1)), []);

  useEffect(() => {
    if (search === q) return;
    const timer = setTimeout(() => set({ q: search || null, page: null }, "push"), 250);
    return () => clearTimeout(timer);
  }, [search, q, set]);

  // The server clamps an out-of-range page. Put that page in the URL.
  useEffect(() => {
    if (data && data.page !== page) set({ page: data.page === 1 ? null : String(data.page) });
  }, [data, page, set]);

  const loading = loadedKey !== serverQuery;
  const rows = data?.items ?? [];
  const quotas = data?.quotas ?? [];
  const activeFilters = [
    !projectId && project ? { key: "project", label: projects.find((item) => item.project_id === project)?.name ?? "Project" } : null,
    kind ? { key: "kind", label: kind } : null,
    status ? { key: "status", label: status } : null,
    trust ? { key: "trust", label: TRUST_LABELS[trust as TrustLevel] ?? trust } : null,
    branch ? { key: "branch", label: branch } : null,
  ].filter((chip): chip is { key: string; label: string } => chip !== null);
  const projectOptions = projects.map((item) => ({ value: item.project_id, label: item.archived_at != null ? `${item.name} (archived)` : item.name }));
  const branchOptions = (branches.project === project ? branches.rows : []).map((item) => ({ value: item.branch, label: item.branch }));

  function resetPage(patch: Record<string, string | null>) {
    set({ ...patch, page: null });
  }

  async function movePin(rows: ConsoleMemory[], index: number, direction: -1 | 1) {
    const next = rows.slice();
    const target = index + direction;
    const moved = next[index];
    if (!moved || target < 0 || target >= next.length) return;
    next.splice(index, 1);
    next.splice(target, 0, moved);
    setPinning(moved.id);
    try {
      await cairn("memories/pins/order", {
        method: "POST",
        body: JSON.stringify({
          scope: moved.scope,
          project_id: moved.project_id,
          order: next.map((row) => ({ id: row.id, position: row.pin_position })),
        }),
      });
      announceMemoriesChanged();
    } catch (cause) {
      if (cause instanceof SessionRequiredError) requireSignIn();
      else if (cause instanceof ApiError && cause.status === 409) {
        announceMemoriesChanged();
        setError("Pin order changed. The list was reloaded.");
      } else {
        setError(cause instanceof Error ? cause.message : "Could not reorder pins.");
      }
    } finally {
      setPinning(null);
    }
  }

  async function togglePin(memory: ConsoleMemory) {
    if (!canTogglePin(memory)) return;
    const quota = quotaFor(memory, quotas);
    const full = quota !== undefined && memory.pinned_at == null && quota.used >= quota.limit;
    if (full) return;
    setPinning(memory.id);
    try {
      await cairn(`memories/${encodeURIComponent(memory.id)}/pin`, {
        method: "POST",
        body: JSON.stringify({ pinned: memory.pinned_at == null }),
      });
      announceMemoriesChanged();
    } catch (cause) {
      if (cause instanceof SessionRequiredError) requireSignIn();
      else if (cause instanceof ApiError && cause.status === 409) {
        setError(cause.message);
      } else {
        setError(cause instanceof Error ? cause.message : "Could not change the pin.");
      }
    } finally {
      setPinning(null);
    }
  }

  const controls = (
    <div className={`grid gap-2 sm:grid-cols-2 ${projectId ? "lg:grid-cols-4" : "lg:grid-cols-5"}`}>
      {!projectId ? (
        <FilterSelect label="Project" value={project} allLabel="All projects" options={projectOptions} onChange={(value) => resetPage({ project: value, branch: null })} />
      ) : null}
      <FilterSelect label="Kind" value={kind} allLabel="All kinds" options={KIND_OPTIONS} onChange={(value) => resetPage({ kind: value })} />
      <FilterSelect label="Status" value={status} allLabel="All statuses" options={statuses} onChange={(value) => resetPage({ status: value })} />
      <FilterSelect label="Trust" value={trust} allLabel="All trust levels" options={TRUST_OPTIONS} onChange={(value) => resetPage({ trust: value })} />
      <FilterSelect label="Branch" value={branch} allLabel={project ? "All branches" : "Choose a project first"} options={branchOptions} disabled={!project} onChange={(value) => resetPage({ branch: value })} />
    </div>
  );

  const groups = view === "pinned" ? groupPins(rows, quotas) : [{ key: "all", title: "", quota: undefined, rows }];

  return (
    <div className="flex min-w-0 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <InputGroup className="min-w-0 flex-1 sm:max-w-sm">
          <InputGroupAddon>
            <SearchIcon />
          </InputGroupAddon>
          <InputGroupInput
            ref={searchRef}
            type="search"
            aria-label="Search memories"
            placeholder="Search memories"
            value={search}
            onChange={(event) => {
              const value = event.target.value;
              setSearch(value);
              if (!value) set({ q: null, page: null }, "push");
            }}
          />
        </InputGroup>
        <label className="flex items-center gap-2 text-sm">
          <span className="sr-only">Match</span>
          <select
            className="h-11 rounded-md border bg-background px-2"
            aria-label="Match"
            value={match}
            onChange={(event) => resetPage({ match: event.target.value === "exact" ? null : event.target.value })}
          >
            <option value="exact">Exact</option>
            <option value="related">Related</option>
          </select>
        </label>
        {mobile ? (
          <Button type="button" variant="outline" className="min-h-11" onClick={() => setFiltersOpen(true)}>
            <FilterIcon />
            Filters{activeFilters.length > 0 ? ` ${activeFilters.length}` : ""}
          </Button>
        ) : null}
      </div>
      <Tabs value={view} onValueChange={(value) => resetPage({ view: value === "all" ? null : value })}>
        <TabsList>
          <TabsTrigger value="all">All</TabsTrigger>
          <TabsTrigger value="pinned">Pinned</TabsTrigger>
        </TabsList>
      </Tabs>
      {mobile ? (
        <Sheet open={filtersOpen} onOpenChange={setFiltersOpen}>
          <SheetContent side="bottom">
            <SheetHeader>
              <SheetTitle>Filters</SheetTitle>
              <SheetDescription>Narrow the memories list.</SheetDescription>
            </SheetHeader>
            <div className="px-4 pb-4">{controls}</div>
          </SheetContent>
        </Sheet>
      ) : (
        controls
      )}
      {activeFilters.length > 0 ? (
        <div className="flex flex-wrap items-center gap-2">
          {activeFilters.map((chip) => (
            <Button key={chip.key} type="button" variant="outline" size="sm" onClick={() => resetPage({ [chip.key]: null, ...(chip.key === "project" ? { branch: null } : {}) })}>
              {chip.label}
              <XIcon />
              <span className="sr-only">Remove {chip.label}</span>
            </Button>
          ))}
        </div>
      ) : null}
      {error ? <ErrorAlert message={error} onRetry={() => setAttempt((n) => n + 1)} /> : null}
      {!projectId || view === "pinned" ? <PinSummaryBar projectId={project || null} /> : null}
      <p className="sr-only" aria-live="polite">
        {loading ? "Loading memories" : data ? `Showing ${data.page_size === 0 ? 0 : Math.min(data.total, (data.page - 1) * data.page_size + 1)} to ${Math.min(data.page * data.page_size, data.total)} of ${data.total}` : ""}
      </p>
      {loading && !data ? <LoadingBlock label="Loading memories…" rows={8} /> : null}
      {data && rows.length === 0 ? (
        <Empty className="border">
          <EmptyHeader>
            <EmptyMedia variant="icon">
              <BrainIcon />
            </EmptyMedia>
            <EmptyTitle>{q || activeFilters.length > 0 || view === "pinned" ? "No memories match" : "No memories yet"}</EmptyTitle>
            <EmptyDescription>
              {q && match === "exact" ? "No exact phrase. Related search uses the usual word matches." : view === "pinned" ? "Pin a memory from All." : "Agents save memories as they work."}
            </EmptyDescription>
          </EmptyHeader>
          {q && match === "exact" ? (
            <EmptyContent>
              <Button type="button" onClick={() => resetPage({ match: "related" })}>
                Search related terms
              </Button>
            </EmptyContent>
          ) : null}
        </Empty>
      ) : null}
      <div aria-busy={loading || undefined} className={cn("flex flex-col gap-4", loading && data ? "opacity-70" : "")}>
        {groups.map((group) => (
          <section key={group.key} className="flex flex-col gap-1">
            {group.title ? (
              <h2 className="flex flex-wrap items-baseline justify-between gap-2 text-sm">
                <span className="font-medium">{group.title}</span>
                {group.quota ? (
                  <span className="tabular-nums text-muted-foreground">
                    {group.quota.used} / {group.quota.limit}
                    {group.quota.scope === "project" ? " shared" : ""}
                  </span>
                ) : null}
              </h2>
            ) : null}
            <ul className="divide-y rounded-lg border bg-card">
              {group.rows.map((memory, index) => {
                const quota = quotaFor(memory, quotas);
                const full = quota !== undefined && memory.pinned_at == null && quota.used >= quota.limit;
                const toggle = canTogglePin(memory);
                const reorder = view === "pinned"
                  && !q
                  && activeFilters.length === 0
                  && group.quota !== undefined
                  && group.rows.length === group.quota.used
                  && group.rows.every((row) => row.can_reorder && row.pin_position != null);
                return (
                  <li key={memory.id} className="flex items-start gap-2 px-3 py-2">
                    {toggle ? (
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon"
                        className="mt-0.5 min-h-11 min-w-11 shrink-0"
                        aria-pressed={memory.pinned_at != null}
                        aria-label={memory.pinned_at != null ? `Unpin ${memory.title}` : `Pin ${memory.title}`}
                        disabled={pinning === memory.id || full}
                        onClick={() => void togglePin(memory)}
                      >
                        <Pin className={memory.pinned_at != null ? "fill-brand text-brand" : "text-muted-foreground"} />
                      </Button>
                    ) : memory.pinned_at != null ? (
                      <span className="mt-3 inline-flex min-h-11 min-w-11 items-center justify-center" title="Pinned">
                        <Pin aria-hidden className="fill-brand text-brand" />
                        <span className="sr-only">Pinned</span>
                      </span>
                    ) : (
                      <span className="w-11 shrink-0" />
                    )}
                    <div className="min-w-0 flex-1 py-1">
                      <button
                        type="button"
                        className="rounded-sm text-left text-sm font-medium break-words underline-offset-4 outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
                        onClick={() => set({ memory: memory.id, field: memory.matched?.field ?? null }, "push")}
                      >
                        {memory.title}
                      </button>
                      {memory.matched ? <MatchExcerpt match={memory.matched} /> : <p className="line-clamp-2 text-sm text-muted-foreground">{memory.summary}</p>}
                      <p className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
                        <KindBadge kind={memory.kind} />
                        <span>{memory.project_name ?? (memory.scope === "global" ? "Global" : "Personal")}</span>
                        {memory.context.branch ? <span className="font-mono">{memory.context.branch}</span> : null}
                        <StatusBadge status={memory.status} cause={memory.stale_cause} />
                        {memory.status === "superseded" ? <span>Superseded</span> : null}
                        <Time value={view === "pinned" ? memory.pinned_at : memory.updated_at} />
                      </p>
                      {full && quota ? (
                        <Button
                          type="button"
                          variant="link"
                          className="h-auto px-0"
                          onClick={() => resetPage({ view: "pinned", ...(quota.project_id && !projectId ? { project: quota.project_id } : {}) })}
                        >
                          Manage pins ({quota.used}/{quota.limit})
                        </Button>
                      ) : null}
                    </div>
                    {reorder ? (
                      <div className="flex shrink-0 flex-col gap-1 py-1">
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          disabled={index === 0 || pinning === memory.id}
                          aria-label={`Move ${memory.title} up`}
                          onClick={() => void movePin(group.rows, index, -1)}
                        >
                          Up
                        </Button>
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          disabled={index === group.rows.length - 1 || pinning === memory.id}
                          aria-label={`Move ${memory.title} down`}
                          onClick={() => void movePin(group.rows, index, 1)}
                        >
                          Down
                        </Button>
                      </div>
                    ) : null}
                  </li>
                );
              })}
            </ul>
          </section>
        ))}
      </div>
      {data && data.total > 0 ? (
        <PageNav
          page={data.page}
          pageSize={data.page_size}
          total={data.total}
          onPage={(next) => set({ page: next === 1 ? null : String(next) }, "push")}
          onPageSize={(size) => resetPage({ pageSize: size === 25 ? null : String(size) })}
        />
      ) : null}
    </div>
  );
}

function groupPins(rows: ConsoleMemory[], quotas: PinQuota[]) {
  const groups: { key: string; title: string; quota?: PinQuota; rows: ConsoleMemory[] }[] = [];
  for (const memory of rows) {
    const key = memory.scope === "project" ? `project:${memory.project_id}` : memory.scope;
    const title = memory.scope === "project" ? (memory.project_name ?? "Project") : memory.scope === "personal" ? "Personal" : "Global";
    const current = groups.at(-1);
    if (current && current.key === key) current.rows.push(memory);
    else groups.push({ key, title, quota: quotaFor(memory, quotas), rows: [memory] });
  }
  return groups;
}
