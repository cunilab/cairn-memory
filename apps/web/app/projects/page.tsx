"use client";

import { useEffect, useMemo, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import type { ColumnDef } from "@tanstack/react-table";
import { FolderGitIcon } from "lucide-react";
import { IdText, RoleBadge } from "@/components/badges";
import { ConsoleShell } from "@/components/console-shell";
import { DataTable } from "@/components/data-table";
import { PageHeader } from "@/components/page-header";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { cairn } from "@/lib/cairn";
import { formatTime } from "@/lib/report";
import { SessionRequiredError } from "@/lib/session";
import type { ListedProject } from "@/lib/types";

export default function ProjectsPage() {
  const router = useRouter();
  const [projects, setProjects] = useState<ListedProject[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [sessionRequired, setSessionRequired] = useState(false);

  useResetOnHide(() => {
    setProjects([]);
    setError(null);
    setLoading(true);
    setSessionRequired(false);
  });

  useEffect(() => {
    let cancelled = false;
    cairn<ListedProject[]>("projects")
      .then((listed) => {
        if (!cancelled) {
          setProjects(listed);
        }
      })
      .catch((cause: unknown) => {
        if (cancelled) {
          return;
        }
        if (cause instanceof SessionRequiredError) {
          setSessionRequired(true);
          router.replace("/");
          return;
        }
        const message = cause instanceof Error ? cause.message : "Could not load projects";
        setError(`Projects could not be loaded. ${message}`);
      })
      .finally(() => {
        if (!cancelled) {
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [router]);

  const columns = useMemo<ColumnDef<ListedProject>[]>(
    () => [
      {
        accessorKey: "name",
        header: "Project",
        cell: ({ row }) => (
          <div className="flex min-w-0 flex-col gap-0.5">
            <Link
              href={`/projects/${row.original.project_id}`}
              className="w-fit rounded-sm font-medium break-words underline-offset-4 outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
            >
              {row.original.name}
            </Link>
            <span className="font-mono text-xs break-all text-muted-foreground">
              {row.original.repository}
            </span>
          </div>
        ),
      },
      {
        accessorKey: "role",
        header: "Role",
        cell: ({ row }) => <RoleBadge role={row.original.role} />,
      },
      {
        accessorKey: "default_branch",
        header: "Branch",
        cell: ({ row }) =>
          row.original.default_branch ? (
            <span className="font-mono text-xs">{row.original.default_branch}</span>
          ) : (
            <span className="text-muted-foreground">—</span>
          ),
        meta: { className: "hidden md:table-cell" },
      },
      {
        accessorKey: "project_id",
        header: "ID",
        cell: ({ row }) => <IdText id={row.original.project_id} />,
        enableSorting: false,
        meta: { className: "hidden lg:table-cell" },
      },
      {
        accessorKey: "updated_at",
        header: "Updated",
        sortingFn: (a, b) => Number(a.original.updated_at) - Number(b.original.updated_at),
        cell: ({ row }) => (
          <span className="text-xs whitespace-nowrap text-muted-foreground">
            {formatTime(row.original.updated_at)}
          </span>
        ),
        meta: { className: "hidden md:table-cell" },
      },
    ],
    [],
  );

  if (sessionRequired) {
    return (
      <main className="mx-auto flex min-h-svh w-full max-w-sm flex-1 items-center px-6">
        <p className="text-sm text-muted-foreground">Sign in is required.</p>
      </main>
    );
  }

  return (
    <ConsoleShell crumbs={[{ label: "Projects" }]}>
      <PageHeader
        title="Projects"
        description="Projects you belong to. Every member can read the project decisions."
      />
      {error ? (
        <Alert variant="destructive">
          <AlertDescription className="text-destructive">{error}</AlertDescription>
        </Alert>
      ) : null}
      {loading ? (
        <div role="status" className="flex flex-col gap-2">
          <span className="sr-only">Loading projects…</span>
          <Skeleton className="h-8 w-64" />
          <Skeleton className="h-48 w-full" />
        </div>
      ) : null}
      {!loading && !error ? (
        <DataTable
          label="projects"
          data={projects}
          columns={columns}
          getRowId={(project) => project.project_id}
          filterPlaceholder="Filter projects"
          initialSort={[{ id: "updated_at", desc: true }]}
          empty={
            <Empty className="border-0 p-6">
              <EmptyHeader>
                <EmptyMedia variant="icon">
                  <FolderGitIcon />
                </EmptyMedia>
                <EmptyTitle>No projects</EmptyTitle>
                <EmptyDescription>
                  No projects for this account. An agent registers one when it first works in a
                  repository with one of your devices.
                </EmptyDescription>
              </EmptyHeader>
              <EmptyContent>
                <Button nativeButton={false} render={<Link href="/devices" />}>Set up a device</Button>
              </EmptyContent>
            </Empty>
          }
        />
      ) : null}
    </ConsoleShell>
  );
}
