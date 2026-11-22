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
import { ErrorAlert, LoadingBlock } from "@/components/states";
import { Time } from "@/components/time";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useAccount } from "@/hooks/use-account";
import { useOpenMemory, useTabParam } from "@/hooks/use-query-params";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { ApiError, cairn } from "@/lib/cairn";
import { LOGIN_PATH, SessionRequiredError } from "@/lib/session";
import type { BranchSummary } from "@/lib/generated/BranchSummary";
import type { MemoryView } from "@/lib/generated/MemoryView";
import type { ProjectMember } from "@/lib/generated/ProjectMember";
import type { ProjectView } from "@/lib/generated/ProjectView";
import { ProjectBranches } from "./project-branches";

const TABS = ["branches", "stale", "members", "instructions"] as const;

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
  const [tab, setTab] = useTabParam(TABS);
  const generation = useRef(0);
  const [data, setData] = useState<Loaded | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notFound, setNotFound] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [renaming, setRenaming] = useState(false);
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
    setError(null);
    setNotFound(false);
    setRenaming(false);
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
          router.replace(LOGIN_PATH);
          return;
        }
        if (cause instanceof ApiError && cause.status === 404) setNotFound(true);
        else setError(`Project could not be loaded. ${cause instanceof Error ? cause.message : "Check that the server is running, then retry."}`);
      });
    return () => {
      cancelled = true;
    };
  }, [fetchProject, router, attempt]);

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
          router.replace(LOGIN_PATH);
          return;
        }
        setData(null);
        setError(`${success}. The page could not reload. Refresh it; do not repeat the action.`);
      }
    },
    [fetchProject, router],
  );

  const owner = data?.project.role === "owner";
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
  }, [canTransfer]);

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
        title={data?.project.name ?? "Project"}
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
          owner ? (
            <Button type="button" variant="outline" onClick={() => setRenaming(true)}>
              Rename
            </Button>
          ) : null
        }
      />
      {error ? (
        <ErrorAlert message={error} onRetry={() => { setError(null); setData(null); setAttempt((n) => n + 1); }} />
      ) : null}
      {data === null && !error ? <LoadingBlock label="Loading project…" rows={4} /> : null}
      {data ? (
        <Tabs value={tab} onValueChange={(value) => setTab(value as (typeof TABS)[number])}>
          <TabsList>
            <TabsTrigger value="branches">Branches</TabsTrigger>
            <TabsTrigger value="stale">Stale ({data.stale.length})</TabsTrigger>
            <TabsTrigger value="members">Members</TabsTrigger>
            <TabsTrigger value="instructions">Agent instructions</TabsTrigger>
          </TabsList>
          <TabsContent value="branches">
            <ProjectBranches branches={data.branches} />
          </TabsContent>
          <TabsContent value="stale">
            <DataTable
              label="stale memories"
              data={data.stale}
              columns={staleColumns}
              getRowId={(m) => m.id}
              filterPlaceholder="Filter stale memories"
              empty={<p className="p-6 text-sm text-muted-foreground">Nothing is stale in this project.</p>}
            />
          </TabsContent>
          <TabsContent value="members">
            <DataTable
              label="members"
              data={data.members}
              columns={memberColumns}
              getRowId={(m) => m.person_id}
              filterPlaceholder="Filter members"
              empty={<p className="p-6 text-sm text-muted-foreground">No members found.</p>}
            />
          </TabsContent>
          <TabsContent value="instructions">
            <AgentInstructionsEditor
              path={`${path}/agent-instructions`}
              scope="project"
              canEdit={canTransfer}
            />
          </TabsContent>
        </Tabs>
      ) : null}
      <FormDialog
        open={renaming}
        onOpenChange={setRenaming}
        title="Rename project"
        description="Agents keep working; the name is only a label."
        submitLabel="Rename"
        onSubmit={async (form) => {
          const name = String(form.get("name")).trim();
          if (name.length < 1 || name.length > 80) throw new Error("Use a name of 1 to 80 characters.");
          await submit(() => cairn(`${path}/rename`, { method: "POST", body: JSON.stringify({ name }) }), "Project renamed");
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
