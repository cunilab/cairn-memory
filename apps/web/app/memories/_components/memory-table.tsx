"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import type { ColumnDef } from "@tanstack/react-table";
import { BrainIcon, FilterIcon, SearchIcon, XIcon } from "lucide-react";
import { KindBadge, StatusBadge } from "@/components/badges";
import { DataTable } from "@/components/data-table";
import { FilterSelect, type FilterOption } from "@/components/filter-select";
import { ErrorAlert, LoadingBlock } from "@/components/states";
import { Time } from "@/components/time";
import { Button } from "@/components/ui/button";
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { useIsMobile } from "@/hooks/use-mobile";
import { useOpenMemory, useQueryParams } from "@/hooks/use-query-params";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { cairn, cairnPage } from "@/lib/cairn";
import { useAccount } from "@/hooks/use-account";
import { memoryLabel } from "./memory-label";
import { onMemoriesChanged } from "@/lib/memory-events";
import { LOGIN_PATH, SessionRequiredError } from "@/lib/session";
import type { BranchSummary } from "@/lib/generated/BranchSummary";
import type { MemoryView } from "@/lib/generated/MemoryView";
import type { ProjectView } from "@/lib/generated/ProjectView";

const PAGE = 50;
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

type Page = { key: string; items: MemoryView[]; next: string | null; error: string | null };

export function MemoryTable() {
  const router = useRouter();
  const mobile = useIsMobile();
  const openMemory = useOpenMemory();
  const account = useAccount();
  const statuses: FilterOption[] = account?.is_admin
    ? [...STATUSES, { value: "forgotten", label: "Forgotten" }]
    : STATUSES;
  const [params, set] = useQueryParams();
  const project = params.get("project") ?? "";
  const kind = params.get("kind") ?? "";
  const status = params.get("status") ?? "";
  const branch = params.get("branch") ?? "";
  const q = params.get("q") ?? "";
  const [search, setSearch] = useState(q);
  const [projects, setProjects] = useState<ProjectView[]>([]);
  const [branches, setBranches] = useState<{ project: string; rows: BranchSummary[] }>({ project: "", rows: [] });
  const [page, setPage] = useState<Page | null>(null);
  const [more, setMore] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [filtersOpen, setFiltersOpen] = useState(false);

  const serverQuery = useMemo(() => {
    const query = new URLSearchParams({ limit: String(PAGE) });
    if (project) query.set("projectId", project);
    if (kind) query.set("kind", kind);
    if (status) query.set("status", status);
    if (branch) query.set("branch", branch);
    if (q.trim()) query.set("q", q.trim());
    return query.toString();
  }, [project, kind, status, branch, q]);
  const key = `${serverQuery}#${attempt}`;
  useResetOnHide(() => {
    setPage(null);
    setProjects([]);
    setBranches({ project: "", rows: [] });
  });
  const loading = page?.key !== key;

  const requireSignIn = useCallback(() => router.replace(LOGIN_PATH), [router]);

  useEffect(() => {
    let cancelled = false;
    cairn<ProjectView[]>("projects")
      .then((rows) => !cancelled && setProjects(rows))
      .catch((cause: unknown) => {
        if (!cancelled && cause instanceof SessionRequiredError) requireSignIn();
      });
    return () => {
      cancelled = true;
    };
  }, [requireSignIn]);

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
    cairnPage<MemoryView>(`memories?${serverQuery}`)
      .then(({ items, next }) => !cancelled && setPage({ key, items, next, error: null }))
      .catch((cause: unknown) => {
        if (cancelled) return;
        if (cause instanceof SessionRequiredError) {
          requireSignIn();
          return;
        }
        setPage({
          key,
          items: [],
          next: null,
          error: `Memories could not be loaded. ${cause instanceof Error ? cause.message : "Check that the server is running, then retry."}`,
        });
      });
    return () => {
      cancelled = true;
    };
  }, [key, serverQuery, requireSignIn]);

  useEffect(() => onMemoriesChanged(() => setAttempt((n) => n + 1)), []);

  // Debounce the search box into the URL.
  useEffect(() => {
    if (search === q) return;
    const timer = setTimeout(() => set({ q: search }), 250);
    return () => clearTimeout(timer);
  }, [search, q, set]);

  async function loadMore() {
    if (!page?.next) return;
    setMore(true);
    try {
      const next = await cairnPage<MemoryView>(`memories?${serverQuery}&cursor=${encodeURIComponent(page.next)}`);
      setPage((current) =>
        current && current.key === key
          ? { ...current, items: [...current.items, ...next.items], next: next.next }
          : current,
      );
    } catch (cause) {
      if (cause instanceof SessionRequiredError) requireSignIn();
      else setPage((current) => (current ? { ...current, error: "More memories could not be loaded. Try again." } : current));
    } finally {
      setMore(false);
    }
  }

  const rows = page?.items ?? [];

  const active = [project, kind, status, branch, q].filter(Boolean).length;
  const projectOptions = projects.map((p) => ({ value: p.project_id, label: p.name }));
  const branchOptions = (branches.project === project ? branches.rows : []).map((b) => ({ value: b.branch, label: b.branch }));

  const columns = useMemo<ColumnDef<MemoryView>[]>(
    () => [
      {
        accessorKey: "kind",
        header: "Kind",
        cell: ({ row }) => <KindBadge kind={row.original.kind} />,
      },
      {
        id: "title",
        header: "Title",
        accessorFn: (m) => m.title,
        cell: ({ row }) => (
          <div className="flex min-w-0 flex-col gap-1">
            <button
              type="button"
              aria-haspopup="dialog"
              className="w-fit rounded-sm text-left text-sm font-medium break-words underline-offset-4 outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
              onClick={() => openMemory(row.original.id)}
            >
              {row.original.title}
            </button>
            <p className="line-clamp-2 text-sm text-muted-foreground">{row.original.summary}</p>
          </div>
        ),
      },
      {
        id: "project",
        header: "Project",
        accessorFn: (m) => m.project_name ?? m.scope,
        cell: ({ row }) => {
          const m = row.original;
          if (m.project_id) return m.project_name ?? "—";
          return m.scope === "global" ? "Global" : "Personal";
        },
        meta: { className: "hidden md:table-cell" },
      },
      {
        accessorKey: "status",
        header: "Status",
        cell: ({ row }) => {
          const label = memoryLabel({
            id: row.original.id,
            status: row.original.status,
            stale_cause: row.original.stale_cause,
            wrong: row.original.usage.wrong,
            links: row.original.links,
          });
          return (
            <div className="flex flex-col items-start gap-1">
              <StatusBadge status={row.original.status} cause={row.original.stale_cause} />
              {label ? (
                label.id ? (
                  <button
                    type="button"
                    className="rounded-sm text-left text-xs text-muted-foreground underline-offset-4 outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
                    onClick={() => openMemory(label.id as string)}
                  >
                    {label.text}
                  </button>
                ) : (
                  <span className="text-xs text-muted-foreground">{label.text}</span>
                )
              ) : null}
            </div>
          );
        },
      },
      {
        accessorKey: "fetched",
        header: "Used",
        cell: ({ row }) => <span className="text-sm tabular-nums">{row.original.fetched.toLocaleString()}</span>,
        meta: { className: "hidden md:table-cell" },
      },
      {
        accessorKey: "updated_at",
        header: "Updated",
        cell: ({ row }) => <Time value={row.original.updated_at} className="text-xs whitespace-nowrap text-muted-foreground" />,
        meta: { className: "hidden md:table-cell" },
      },
    ],
    [openMemory],
  );

  const controls = (
    <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
      <FilterSelect label="Project" value={project} allLabel="All projects" options={projectOptions} onChange={(v) => set({ project: v, branch: null })} />
      <FilterSelect label="Kind" value={kind} allLabel="All kinds" options={KIND_OPTIONS} onChange={(v) => set({ kind: v })} />
      <FilterSelect label="Status" value={status} allLabel="All statuses" options={statuses} onChange={(v) => set({ status: v })} />
      <FilterSelect label="Branch" value={branch} allLabel={project ? "All branches" : "Choose a project first"} options={branchOptions} disabled={!project} onChange={(v) => set({ branch: v })} />
    </div>
  );

  return (
    <div className="flex min-w-0 flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <InputGroup className="w-full sm:max-w-xs">
          <InputGroupAddon>
            <SearchIcon />
          </InputGroupAddon>
          <InputGroupInput
            type="search"
            aria-label="Search title or summary"
            placeholder="Search title or summary…"
            value={search}
            onChange={(event) => {
              const value = event.target.value;
              setSearch(value);
              // Clearing applies at once; typing is debounced below.
              if (!value) set({ q: null });
            }}
          />
        </InputGroup>
        {mobile ? (
          <Button type="button" variant="outline" className="min-h-11" onClick={() => setFiltersOpen(true)}>
            <FilterIcon />
            Filters ({active})
          </Button>
        ) : null}
      </div>
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
      {active > 0 ? (
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <span>
            {active} {active === 1 ? "filter" : "filters"}
          </span>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={() => {
              setSearch("");
              set({ project: null, kind: null, status: null, branch: null, q: null });
            }}
          >
            <XIcon />
            Clear
          </Button>
        </div>
      ) : null}
      {page?.error ? <ErrorAlert message={page.error} onRetry={() => setAttempt((n) => n + 1)} /> : null}
      {loading ? (
        <LoadingBlock label="Loading memories…" rows={8} />
      ) : (
        <DataTable
          label="memories"
          data={rows}
          columns={columns}
          getRowId={(m) => m.id}
          filterPlaceholder="Filter memories"
          searchable={false}
          pageSize={PAGE}
          empty={
            <Empty className="border-0 p-6">
              <EmptyHeader>
                <EmptyMedia variant="icon">
                  <BrainIcon />
                </EmptyMedia>
                <EmptyTitle>{active > 0 ? "No memories match these filters" : "No memories yet"}</EmptyTitle>
                <EmptyDescription>
                  {active > 0
                    ? "Try fewer filters."
                    : "Agents save memories as they work. Finish setup in Settings › Devices."}
                </EmptyDescription>
              </EmptyHeader>
              <EmptyContent>
                {active > 0 ? (
                  <Button
                    type="button"
                    variant="outline"
                    onClick={() => {
                      setSearch("");
                      set({ project: null, kind: null, status: null, branch: null, q: null });
                    }}
                  >
                    Clear filters
                  </Button>
                ) : (
                  <Button nativeButton={false} render={<Link href="/settings?tab=devices" />}>
                    Set up a device
                  </Button>
                )}
              </EmptyContent>
            </Empty>
          }
        />
      )}
      {!loading && page?.next ? (
        <div className="flex justify-center">
          <Button type="button" variant="outline" disabled={more} onClick={() => void loadMore()}>
            {more ? "Loading…" : "Load more"}
          </Button>
        </div>
      ) : null}
    </div>
  );
}
