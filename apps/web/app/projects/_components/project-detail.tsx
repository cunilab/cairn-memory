"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import type { ColumnDef } from "@tanstack/react-table";
import { toast } from "sonner";
import { RoleBadge } from "@/components/badges";
import { AgentInstructionsEditor } from "@/components/agent-instructions-editor";
import { ConsoleShell } from "@/components/console-shell";
import { DataTable } from "@/components/data-table";
import { FormDialog } from "@/components/form-dialog";
import { PageHeader } from "@/components/page-header";
import { PinSummaryBar } from "@/components/pin-summary";
import { ErrorAlert, LoadingBlock } from "@/components/states";
import { Time } from "@/components/time";
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent,
  AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useAccount } from "@/hooks/use-account";
import { useOpenMemory, useQueryParams, useTabParam } from "@/hooks/use-query-params";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { ApiError, cairn } from "@/lib/cairn";
import { announceMemoriesChanged } from "@/lib/memory-events";
import { loginHref, SessionRequiredError } from "@/lib/session";
import type { BranchSummary } from "@/lib/generated/BranchSummary";
import type { MemoryView } from "@/lib/generated/MemoryView";
import type { ProjectMember } from "@/lib/generated/ProjectMember";
import type { ProjectView } from "@/lib/generated/ProjectView";
import type { SessionReport } from "@/lib/generated/SessionReport";
import { MemoryTable } from "../../memories/_components/memory-table";
import { ProjectBranches } from "./project-branches";
import { ProjectSessions } from "./project-sessions";

const TABS = ["overview", "memories", "sessions", "settings", "branches", "stale", "members", "instructions"] as const;

function shownTab(tab: (typeof TABS)[number]): "overview" | "memories" | "sessions" | "settings" {
  if (tab === "memories") return "memories";
  if (tab === "sessions" || tab === "branches") return "sessions";
  if (tab === "settings" || tab === "members" || tab === "instructions") return "settings";
  return "overview";
}

type Loaded = {
  project: ProjectView;
  branches: BranchSummary[];
  stale: MemoryView[];
  members: ProjectMember[];
};

export function ProjectDetail({ projectId }: { projectId: string }) {
  const router = useRouter();
  const account = useAccount();
  const openMemory = useOpenMemory();
  const [params] = useQueryParams();
  const branchFilter = params.get("branch") ?? "";
  const [rawTab, setTab] = useTabParam(TABS);
  const tab = shownTab(rawTab);
  const generation = useRef(0);
  const [projectData, setData] = useState<Loaded | null>(null);
  const [sessions, setSessions] = useState<SessionReport[] | null>(null);
  const [sessionsError, setSessionsError] = useState(false);
  const data = projectData ? { ...projectData, sessions } : null;
  const [error, setError] = useState<string | null>(null);
  const [notFound, setNotFound] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [renaming, setRenaming] = useState(false);
  const [confirmArchive, setConfirmArchive] = useState(false);
  const [archivePending, setArchivePending] = useState(false);
  const [transfer, setTransfer] = useState<ProjectMember | null>(null);
  const path = `projects/${encodeURIComponent(projectId)}`;

  const fetchProject = useCallback(async (): Promise<Loaded> => {
    const listed = await cairn<ProjectView[]>("projects");
    const project = listed.find((p) => p.project_id === projectId);
    if (!project) throw new ApiError("Project not found or you are not a member.", 404);
    const [branches, members, stale] = await Promise.all([
      cairn<BranchSummary[]>(`${path}/branches`),
      cairn<ProjectMember[]>(`${path}/members`),
      cairn<MemoryView[]>(`memories?projectId=${encodeURIComponent(projectId)}&status=stale&limit=100`),
    ]);
    return { project, branches, members, stale };
  }, [projectId, path]);

  useResetOnHide(() => {
    generation.current += 1;
    setData(null);
    setSessions(null);
    setSessionsError(false);
    setError(null);
    setNotFound(false);
    setRenaming(false);
    setConfirmArchive(false);
    setTransfer(null);
  });

  useEffect(() => {
    let cancelled = false;
    const expected = generation.current;
    fetchProject()
      .then((result) => {
        if (cancelled || generation.current !== expected) return;
        setData(result);
        setError(null);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        if (cause instanceof SessionRequiredError) {
          router.replace(loginHref());
          return;
        }
        if (cause instanceof ApiError && cause.status === 404) setNotFound(true);
        else setError(`Project could not be loaded. ${cause instanceof Error ? cause.message : "Check that the server is running, then retry."}`);
      });
    return () => {
      cancelled = true;
    };
  }, [fetchProject, router, attempt]);

  useEffect(() => {
    let cancelled = false;
    const expected = generation.current;
    cairn<SessionReport[]>(`${path}/sessions?days=30`)
      .then((result) => {
        if (!cancelled && generation.current === expected) { setSessions(result); setSessionsError(false); }
      })
      .catch((cause: unknown) => {
        if (cancelled || generation.current !== expected) return;
        if (cause instanceof SessionRequiredError) router.replace(loginHref());
        else setSessionsError(true);
      });
    return () => { cancelled = true; };
  }, [path, router, attempt]);

  const submit = useCallback(
    async (write: () => Promise<unknown>, success: string) => {
      const expected = generation.current;
      await write();
      try {
        const result = await fetchProject();
        if (generation.current === expected) {
          setData(result);
          toast.success(success);
        }
      } catch (cause) {
        if (generation.current !== expected) return;
        if (cause instanceof SessionRequiredError) {
          router.replace(loginHref());
          return;
        }
        setData(null);
        setError(`${success}. The page could not reload. Refresh it; do not repeat the action.`);
      }
    },
    [fetchProject, router],
  );

  async function changeArchive(archived: boolean) {
    setArchivePending(true);
    try {
      await submit(
        () => cairn(`${path}/${archived ? "archive" : "restore"}`, { method: "POST" }),
        archived ? "Project archived" : "Project restored",
      );
      announceMemoriesChanged();
      setConfirmArchive(false);
    } catch (cause) {
      if (cause instanceof SessionRequiredError) router.replace(loginHref());
      else toast.error(cause instanceof Error ? cause.message : "Could not change the project archive.");
    } finally {
      setArchivePending(false);
    }
  }

  const owner = data?.project.role === "owner";
  const archived = data?.project.archived_at != null;
  const archiveCount = data?.project.archive_memory_count ?? 0;
  const canTransfer = owner || account?.is_admin === true;
  const crumbs = [{ label: "Projects", href: "/projects" }, { label: data?.project.name ?? "Project" }];

  const memberColumns = useMemo<ColumnDef<ProjectMember>[]>(() => {
    const cols: ColumnDef<ProjectMember>[] = [
      { id: "person", header: "Email", accessorKey: "email", cell: ({ row }) => <span className="break-all">{row.original.email}</span> },
      { id: "role", header: "Role", cell: ({ row }) => <RoleBadge role={row.original.role} /> },
      {
        id: "joined",
        header: "Joined",
        accessorFn: (m) => m.joined_at,
        cell: ({ row }) => <Time value={row.original.joined_at} className="text-xs whitespace-nowrap text-muted-foreground" />,
        meta: { className: "hidden sm:table-cell" },
      },
    ];
    cols.push({
      id: "active",
      header: "Last active",
      accessorFn: (m) => m.last_active_at ?? 0,
      cell: ({ row }) => <Time value={row.original.last_active_at} className="text-xs whitespace-nowrap text-muted-foreground" />,
      meta: { className: "hidden md:table-cell" },
    });
    cols.push({
      id: "transfer",
      header: () => <span className="sr-only">Actions</span>,
      enableSorting: false,
      cell: ({ row }) =>
        canTransfer && row.original.role !== "owner" ? (
          <Button type="button" variant="outline" size="sm" onClick={() => setTransfer(row.original)}>
            Transfer ownership
          </Button>
        ) : null,
    });
    return cols;
  }, [canTransfer, setTransfer]);

  const staleColumns = useMemo<ColumnDef<MemoryView>[]>(
    () => [
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
            {row.original.pinned_at != null ? <Badge variant="outline">Pinned</Badge> : null}
            <p className="line-clamp-2 text-sm text-muted-foreground">
              {row.original.stale_cause ?? row.original.summary}
            </p>
          </div>
        ),
      },
      {
        accessorKey: "updated_at",
        header: "Updated",
        cell: ({ row }) => <Time value={row.original.updated_at} className="text-xs whitespace-nowrap text-muted-foreground" />,
        meta: { className: "hidden sm:table-cell" },
      },
    ],
    [openMemory],
  );

  if (notFound) {
    return (
      <ConsoleShell crumbs={crumbs}>
        <PageHeader title="Project not found" />
        <Empty className="border">
          <EmptyHeader>
            <EmptyTitle>Project not found or you are not a member</EmptyTitle>
            <EmptyDescription>Check the link, or pick a project from the list.</EmptyDescription>
          </EmptyHeader>
          <Button nativeButton={false} render={<Link href="/projects" />}>
            All projects
          </Button>
        </Empty>
      </ConsoleShell>
    );
  }

  return (
    <ConsoleShell crumbs={crumbs}>
      <PageHeader
        title={
          <span className="flex flex-wrap items-center gap-2">
            {data?.project.name ?? "Project"}
            {archived ? <Badge variant="secondary">Archived</Badge> : null}
          </span>
        }
        description={
          data ? (
            <>
              <code className="font-mono text-xs">{data.project.repository}</code>
              {data.project.default_branch ? ` · default branch ${data.project.default_branch}` : ""}
              {owner ? " · owner you" : ""}
            </>
          ) : (
            "Loading project…"
          )
        }
        actions={
          data && canTransfer ? (
            <>
              {owner ? <Button type="button" variant="outline" onClick={() => setRenaming(true)}>Rename</Button> : null}
              {archived ? (
                <Button type="button" variant="outline" disabled={archivePending} onClick={() => void changeArchive(false)}>Restore</Button>
              ) : (
                <Button type="button" variant="outline" disabled={archivePending} onClick={() => setConfirmArchive(true)}>Archive project</Button>
              )}
            </>
          ) : null
        }
      />
      {error ? (
        <ErrorAlert message={error} onRetry={() => { setError(null); setData(null); setAttempt((n) => n + 1); }} />
      ) : null}
      {data === null && !error ? <LoadingBlock label="Loading project…" rows={4} /> : null}
      {data ? <PinSummaryBar projectId={projectId} /> : null}
      {data ? (
        <Tabs value={tab} onValueChange={(value) => setTab(value as (typeof TABS)[number])}>
          <div className="flex flex-wrap items-center justify-between gap-2">
          <TabsList className="max-w-full flex-wrap justify-start group-data-horizontal/tabs:h-auto [&>[role=tab]]:h-7">
            <TabsTrigger value="overview">Overview</TabsTrigger>
            <TabsTrigger value="memories">Memories</TabsTrigger>
            <TabsTrigger value="sessions">Sessions</TabsTrigger>
            <TabsTrigger value="settings">Settings</TabsTrigger>
          </TabsList>
          <Button nativeButton={false} variant="outline" size="sm" render={<Link href={`/graph?project=${encodeURIComponent(projectId)}`} />}>
            Relationships
          </Button>
          </div>
          <TabsContent value="overview" className="space-y-4">
            <div className="flex flex-wrap gap-2 text-sm">
              <Button nativeButton={false} variant="outline" size="sm" render={<Link href={`/projects/${encodeURIComponent(projectId)}?tab=memories&status=stale`} />}>
                {data.stale.length} stale
              </Button>
              <Button nativeButton={false} variant="outline" size="sm" render={<Link href={`/projects/${encodeURIComponent(projectId)}?tab=memories&view=pinned`} />}>
                Pins
              </Button>
            </div>
            {data.stale.length > 0 ? (
              <DataTable
                label="stale memories"
                data={data.stale.slice(0, 5)}
                columns={staleColumns}
                getRowId={(m) => m.id}
                filterPlaceholder="Filter stale memories"
                empty={<p className="p-6 text-sm text-muted-foreground">Nothing is stale in this project.</p>}
              />
            ) : null}
            {sessionsError ? <ErrorAlert message="Sessions could not be loaded." /> : data.sessions ? <ProjectSessions sessions={data.sessions} initialFilter={branchFilter} /> : <LoadingBlock label="Loading sessions…" />}
          </TabsContent>
          <TabsContent value="memories">
            <MemoryTable projectId={projectId} />
          </TabsContent>
          <TabsContent value="sessions" className="space-y-6">
            <ProjectBranches branches={data.branches} />
            {sessionsError ? <ErrorAlert message="Sessions could not be loaded." /> : data.sessions ? <ProjectSessions sessions={data.sessions} initialFilter={branchFilter} /> : <LoadingBlock label="Loading sessions…" />}
          </TabsContent>
          <TabsContent value="settings" className="space-y-6">
            <DataTable
              label="members"
              data={data.members}
              columns={memberColumns}
              getRowId={(m) => m.person_id}
              filterPlaceholder="Filter members"
              empty={<p className="p-6 text-sm text-muted-foreground">No members found.</p>}
            />
            <AgentInstructionsEditor
              path={`${path}/agent-instructions`}
              scope="project"
              canEdit={canTransfer}
            />
          </TabsContent>
        </Tabs>
      ) : null}
      <AlertDialog open={confirmArchive} onOpenChange={(open) => !archivePending && setConfirmArchive(open)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Archive {data?.project.name}?</AlertDialogTitle>
            <AlertDialogDescription>
              Cairn will hide {archiveCount.toLocaleString()} {archiveCount === 1 ? "memory" : "memories"} from agents.
              The memories stay readable here. Restore the project to make them available to agents again.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={archivePending}>Cancel</AlertDialogCancel>
            <AlertDialogAction disabled={archivePending} onClick={() => void changeArchive(true)}>Archive project</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      <FormDialog
        open={renaming}
        onOpenChange={setRenaming}
        title="Rename project"
        description="Agents keep working; the name is only a label."
        submitLabel="Rename"
        onSubmit={async (form) => {
          const name = String(form.get("name")).trim();
          if (name.length < 1 || name.length > 80) throw new Error("Use a name of 1 to 80 characters.");
          await submit(() => cairn(`${path}/rename`, { method: "POST", body: JSON.stringify({ name }) }), "Update name in .cairn/project.json (or run `cairn init`) and commit it, so teammates see the new name.");
        }}
      >
        <Field>
          <FieldLabel htmlFor="project-name">Name</FieldLabel>
          <Input id="project-name" name="name" defaultValue={data?.project.name} maxLength={80} required autoComplete="off" />
          <FieldDescription>1 to 80 characters.</FieldDescription>
        </Field>
      </FormDialog>
      <FormDialog
        open={transfer !== null}
        onOpenChange={(open) => !open && setTransfer(null)}
        title="Transfer ownership"
        description={`Transfer ${data?.project.name ?? "this project"} to ${transfer?.email ?? "this member"}? You stay a member but can no longer rename or transfer it.`}
        submitLabel="Transfer ownership"
        onSubmit={async () => {
          if (!transfer) return;
          await submit(
            () => cairn(`${path}/ownership`, { method: "POST", body: JSON.stringify({ personId: transfer.person_id }) }),
            "Ownership transferred",
          );
        }}
      >
        <p className="text-sm text-muted-foreground">The new owner can rename the project and transfer it again.</p>
      </FormDialog>
    </ConsoleShell>
  );
}
