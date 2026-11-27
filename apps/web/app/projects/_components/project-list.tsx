"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import type { ColumnDef } from "@tanstack/react-table";
import { toast } from "sonner";
import { FolderGitIcon } from "lucide-react";
import { RoleBadge } from "@/components/badges";
import { CopyButton } from "@/components/copy-button";
import { DataTable } from "@/components/data-table";
import { ErrorAlert, LoadingBlock } from "@/components/states";
import { Time } from "@/components/time";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useAccount } from "@/hooks/use-account";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { cairn } from "@/lib/cairn";
import { announceMemoriesChanged } from "@/lib/memory-events";
import { loginHref, SessionRequiredError } from "@/lib/session";
import type { Project } from "@/lib/generated/Project";
import type { ProjectView } from "@/lib/generated/ProjectView";

export function ProjectList() {
  const router = useRouter();
  const account = useAccount();
  const [tab, setTab] = useState("active");
  const [restoring, setRestoring] = useState<string | null>(null);
  const [projects, setProjects] = useState<ProjectView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useResetOnHide(() => {
    setProjects(null);
    setError(null);
  });

  useEffect(() => {
    let cancelled = false;
    cairn<ProjectView[]>("projects")
      .then((listed) => {
        if (!cancelled) {
          setProjects(listed);
          setError(null);
        }
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        if (cause instanceof SessionRequiredError) {
          router.replace(loginHref());
          return;
        }
        setError(`Projects could not be loaded. ${cause instanceof Error ? cause.message : "Check that the server is running, then retry."}`);
      });
    return () => {
      cancelled = true;
    };
  }, [router, attempt]);

  const restore = useCallback(async (projectId: string) => {
    setRestoring(projectId);
    try {
      const updated = await cairn<Project>(`projects/${encodeURIComponent(projectId)}/restore`, { method: "POST" });
      setProjects((current) => current?.map((p) => p.project_id === projectId ? { ...p, ...updated } : p) ?? null);
      announceMemoriesChanged();
      toast.success("Project restored");
    } catch (cause) {
      if (cause instanceof SessionRequiredError) router.replace(loginHref());
      else toast.error(cause instanceof Error ? cause.message : "Could not restore the project.");
    } finally {
      setRestoring(null);
    }
  }, [router]);

  const columns = useMemo<ColumnDef<ProjectView>[]>(() => {
    const cols: ColumnDef<ProjectView>[] = [
      {
        accessorKey: "name",
        header: "Name",
        cell: ({ row }) => (
          <div className="flex min-w-0 flex-col gap-0.5">
            <Link
              href={`/projects/${encodeURIComponent(row.original.project_id)}`}
              className="w-fit rounded-sm font-medium break-words underline-offset-4 outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
            >
              {row.original.name}
            </Link>
            <span className="text-xs text-muted-foreground md:hidden">
              {row.original.role} ·{" "}
              <Link href={`/projects/${encodeURIComponent(row.original.project_id)}?tab=memories`} className="underline-offset-4 hover:underline">
                {row.original.memories.toLocaleString()} memories
              </Link>
              {" · "}
              <Link href={`/projects/${encodeURIComponent(row.original.project_id)}?tab=memories&status=stale`} className="underline-offset-4 hover:underline">
                {row.original.stale.toLocaleString()} stale
              </Link>
            </span>
          </div>
        ),
      },
      {
        accessorKey: "repository",
        header: "Repository",
        cell: ({ row }) => <span className="font-mono text-xs break-all text-muted-foreground">{row.original.repository}</span>,
        meta: { className: "hidden md:table-cell" },
      },
      {
        accessorKey: "role",
        header: "Role",
        cell: ({ row }) => <RoleBadge role={row.original.role} />,
        meta: { className: "hidden sm:table-cell" },
      },
    ];
    for (const [id, header] of [["memories", "Memories"], ["stale", "Stale"], ["members", "Members"]] as const) {
      cols.push({
        id,
        header,
        accessorFn: (p) => p[id],
        cell: ({ row }) => id === "members" ? (
          <span className="tabular-nums">{row.original[id].toLocaleString()}</span>
        ) : (
          <Link
            href={`/projects/${encodeURIComponent(row.original.project_id)}?tab=memories${id === "stale" ? "&status=stale" : ""}`}
            className="tabular-nums underline-offset-4 hover:underline"
          >
            {row.original[id].toLocaleString()}
          </Link>
        ),
        meta: { className: "hidden md:table-cell" },
      });
    }
    cols.push({
      id: "updated",
      header: "Last activity",
      accessorFn: (p) => p.last_activity_at ?? p.updated_at,
      cell: ({ row }) => (
        <Time value={row.original.last_activity_at ?? row.original.updated_at} className="text-xs whitespace-nowrap text-muted-foreground" />
      ),
      meta: { className: "hidden md:table-cell" },
    });
    if (tab === "archived") cols.push({
      id: "restore",
      header: () => <span className="sr-only">Actions</span>,
      enableSorting: false,
      cell: ({ row }) => (row.original.role === "owner" || account?.is_admin) ? (
        <Button size="sm" variant="outline" disabled={restoring !== null} onClick={() => void restore(row.original.project_id)}>
          {restoring === row.original.project_id ? "Restoring…" : "Restore"}
        </Button>
      ) : null,
    });
    return cols;
  }, [account?.is_admin, restore, restoring, tab]);

  if (error) {
    return <ErrorAlert message={error} onRetry={() => { setError(null); setProjects(null); setAttempt((n) => n + 1); }} />;
  }
  if (projects === null) return <LoadingBlock label="Loading projects…" rows={4} />;
  return (
    <Tabs value={tab} onValueChange={setTab} className="gap-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
      <TabsList>
        <TabsTrigger value="active">Active</TabsTrigger>
        <TabsTrigger value="archived">Archived</TabsTrigger>
      </TabsList>
      <details className="text-sm">
        <summary className="cursor-pointer font-medium">Connect a repository</summary>
        <p className="mt-2 max-w-md text-muted-foreground">
          Run <code className="font-mono">cairn init</code> in the repository. The project id in <code className="font-mono">.cairn/project.json</code> is what joins it.
        </p>
      </details>
      </div>
      <TabsContent value={tab}>
        <DataTable
          label="projects"
          data={projects.filter((project) => (project.archived_at != null) === (tab === "archived"))}
          columns={columns}
          getRowId={(project) => project.project_id}
          filterPlaceholder="Filter projects"
          initialSort={[{ id: "updated", desc: true }]}
          empty={tab === "archived" || projects.length > 0 ? (
            <p className="p-6 text-sm text-muted-foreground">{tab === "archived" ? "No archived projects." : "No active projects."}</p>
          ) : (
            <Empty className="border-0 p-6">
              <EmptyHeader>
                <EmptyMedia variant="icon">
                  <FolderGitIcon />
                </EmptyMedia>
                <EmptyTitle>No projects yet</EmptyTitle>
                <EmptyDescription>
                  Run <code>cairn init</code> in a repository to register it.
                </EmptyDescription>
              </EmptyHeader>
              <EmptyContent>
                <CopyButton text="cairn init" label="Copy command" />
              </EmptyContent>
            </Empty>
          )}
        />
      </TabsContent>
    </Tabs>
  );
}
