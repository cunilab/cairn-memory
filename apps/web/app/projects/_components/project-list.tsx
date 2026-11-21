"use client";

import { useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import type { ColumnDef } from "@tanstack/react-table";
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
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { cairn } from "@/lib/cairn";
import { LOGIN_PATH, SessionRequiredError } from "@/lib/session";
import type { ListedProject } from "@/lib/types";

// TODO(7a): `GET /projects` gains these counts (UX spec §5). Each column appears only when the
// server sends it, so the list works today.
interface ProjectExtras {
  memories?: number;
  stale?: number;
  members?: number;
  last_activity_at?: number | null;
}
type ProjectRow = ListedProject & ProjectExtras;

export function ProjectList() {
  const router = useRouter();
  const [projects, setProjects] = useState<ProjectRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useResetOnHide(() => {
    setProjects(null);
    setError(null);
  });

  useEffect(() => {
    let cancelled = false;
    cairn<ProjectRow[]>("projects")
      .then((listed) => {
        if (!cancelled) {
          setProjects(listed);
          setError(null);
        }
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        if (cause instanceof SessionRequiredError) {
          router.replace(LOGIN_PATH);
          return;
        }
        setError(`Projects could not be loaded. ${cause instanceof Error ? cause.message : "Check that the server is running, then retry."}`);
      });
    return () => {
      cancelled = true;
    };
  }, [router, attempt]);

  const counts = projects?.some((p) => p.memories !== undefined) ?? false;
  const columns = useMemo<ColumnDef<ProjectRow>[]>(() => {
    const cols: ColumnDef<ProjectRow>[] = [
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
            <span className="font-mono text-xs break-all text-muted-foreground md:hidden">{row.original.repository}</span>
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
    if (counts) {
      for (const [id, header] of [["memories", "Memories"], ["stale", "Stale"], ["members", "Members"]] as const) {
        cols.push({
          id,
          header,
          accessorFn: (p) => p[id] ?? 0,
          cell: ({ row }) => <span className="tabular-nums">{row.original[id] ?? "—"}</span>,
          meta: { className: "hidden md:table-cell" },
        });
      }
    }
    cols.push({
      id: "updated",
      header: counts ? "Last activity" : "Updated",
      accessorFn: (p) => Number(p.last_activity_at ?? p.updated_at),
      cell: ({ row }) => (
        <Time value={row.original.last_activity_at ?? row.original.updated_at} className="text-xs whitespace-nowrap text-muted-foreground" />
      ),
      meta: { className: "hidden md:table-cell" },
    });
    return cols;
  }, [counts]);

  if (error) {
    return <ErrorAlert message={error} onRetry={() => { setError(null); setProjects(null); setAttempt((n) => n + 1); }} />;
  }
  if (projects === null) return <LoadingBlock label="Loading projects…" rows={4} />;
  return (
    <DataTable
      label="projects"
      data={projects}
      columns={columns}
      getRowId={(project) => project.project_id}
      filterPlaceholder="Filter projects"
      initialSort={[{ id: "updated", desc: true }]}
      empty={
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
      }
    />
  );
}
