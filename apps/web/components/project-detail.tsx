"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import type { ColumnDef } from "@tanstack/react-table";
import { MoreHorizontalIcon } from "lucide-react";
import { toast } from "sonner";
import { IdText, RoleBadge } from "@/components/badges";
import { ConsoleShell } from "@/components/console-shell";
import { DataTable } from "@/components/data-table";
import { FormDialog } from "@/components/form-dialog";
import { PageHeader } from "@/components/page-header";
import { ProjectBranches } from "@/components/project-branches";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useAccount } from "@/hooks/use-account";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { deleteDecisionImpact } from "@/lib/action-copy";
import { cairn } from "@/lib/cairn";
import { formatTime } from "@/lib/report";
import { SessionRequiredError } from "@/lib/session";
import { MemoryFeedback } from "@/components/memory-feedback";
import type { ListedProject, ProjectBranch, ProjectMember, ProjectMemory } from "@/lib/types";

type Action = { kind: "delete"; memory: ProjectMemory };

export function ProjectDetail({ projectId }: { projectId: string }) {
  const router = useRouter();
  const generation = useRef(0);
  const account = useAccount();
  const [project, setProject] = useState<ListedProject | null>(null);
  const [decisions, setDecisions] = useState<ProjectMemory[]>([]);
  const [branches, setBranches] = useState<ProjectBranch[]>([]);
  const [members, setMembers] = useState<ProjectMember[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [action, setAction] = useState<Action | null>(null);
  const [detail, setDetail] = useState<ProjectMemory | null>(null);
  const [transfer, setTransfer] = useState<ProjectMember | null>(null);
  const path = `projects/${encodeURIComponent(projectId)}`;
  const requireSignIn = useCallback(() => router.replace("/"), [router]);

  const fetchProject = useCallback(async () => {
    const listed = await cairn<ListedProject[]>("projects");
    const current = listed.find((p) => p.project_id === projectId);
    if (!current) throw new Error("Project is not on this account.");
    const [nextDecisions, nextMembers, nextBranches] = await Promise.all([
      cairn<ProjectMemory[]>(`${path}/memories`),
      cairn<ProjectMember[]>(`${path}/members`),
      cairn<ProjectBranch[]>(`${path}/branches`),
    ]);
    return { current, nextDecisions, nextMembers, nextBranches };
  }, [projectId, path]);

  const apply = useCallback((result: Awaited<ReturnType<typeof fetchProject>>) => {
    setBranches(result.nextBranches);
    setProject(result.current);
    setDecisions(result.nextDecisions);
    setMembers(result.nextMembers);
    setError(null);
  }, []);

  useResetOnHide(() => {
    generation.current += 1;
    setBranches([]);
    setProject(null);
    setDecisions([]);
    setMembers([]);
    setError(null);
    setAction(null);
    setDetail(null);
    setTransfer(null);
  });

  useEffect(() => {
    let cancelled = false;
    const expected = generation.current;
    fetchProject()
      .then((result) => {
        if (!cancelled && generation.current === expected) apply(result);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        if (cause instanceof SessionRequiredError) {
          requireSignIn();
          return;
        }
        setError(cause instanceof Error ? cause.message : "Could not load project");
      });
    return () => {
      cancelled = true;
    };
  }, [fetchProject, apply, requireSignIn]);

  const submit = useCallback(
    async (write: () => Promise<unknown>, success: string) => {
      const expected = generation.current;
      await write();
      try {
        const result = await fetchProject();
        if (generation.current === expected) {
          apply(result);
          toast.success(success);
        }
      } catch (cause) {
        if (generation.current !== expected) return;
        if (cause instanceof SessionRequiredError) {
          requireSignIn();
          return;
        }
        setDecisions([]);
        setBranches([]);
        setMembers([]);
        setError(`${success} Reload failed. Refresh this page; do not repeat the action.`);
      }
    },
    [apply, fetchProject, requireSignIn],
  );
  const owner = project?.role === "owner";
  const nameOf = useCallback(
    (id?: string | null) => members.find((m) => m.person_id === id)?.email ?? id ?? "—",
    [members],
  );

  const columns = useMemo<ColumnDef<ProjectMemory>[]>(
    () => [
      {
        id: "decision",
        header: "Memory",
        accessorFn: (m) => `${m.id} ${m.title} ${m.summary}`,
        enableSorting: false,
        cell: ({ row }) => (
          <div className="flex min-w-0 flex-col gap-1.5">
            <button
              type="button"
              aria-haspopup="dialog"
              className="line-clamp-2 text-left text-sm break-words hover:underline focus-visible:ring-2"
              onClick={() => setDetail(row.original)}
            >
              {row.original.title}
            </button>
            <p className="line-clamp-2 text-sm text-muted-foreground">{row.original.summary}</p>
            <span className="text-xs text-muted-foreground">Project</span>
            <IdText id={row.original.id} />
          </div>
        ),
      },
      {
        id: "author",
        header: "Author",
        accessorFn: (m) => nameOf(m.owner),
        meta: { className: "hidden md:table-cell" },
      },
      {
        id: "status",
        header: "Status",
        accessorFn: (m) => m.status,
        meta: { className: "hidden lg:table-cell" },
      },
      {
        id: "created",
        header: "Published",
        accessorFn: (m) => m.created_at,
        cell: ({ row }) => (
          <span className="text-xs text-muted-foreground">
            {formatTime(row.original.created_at)}
          </span>
        ),
        meta: { className: "hidden md:table-cell" },
      },
      {
        id: "actions",
        header: () => <span className="sr-only">Actions</span>,
        enableSorting: false,
        cell: ({ row }) => {
          const memory = row.original;
          // Forgetting erases the text for every member, so only the memory's author may.
          if (memory.owner !== account?.person_id) return null;
          return (
            <DropdownMenu>
              <DropdownMenuTrigger
                render={
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    aria-label={`Actions for ${memory.id}`}
                  />
                }
              >
                <MoreHorizontalIcon />
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem
                  variant="destructive"
                  onClick={() => setAction({ kind: "delete", memory })}
                >
                  Forget
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          );
        },
      },
    ],
    [account?.person_id, nameOf],
  );

  const memberColumns = useMemo<ColumnDef<ProjectMember>[]>(
    () => [
      { id: "person", header: "Member", accessorKey: "email" },
      { id: "role", header: "Role", cell: ({ row }) => <RoleBadge role={row.original.role} /> },
      {
        id: "transfer",
        header: "",
        enableSorting: false,
        cell: ({ row }) =>
          (owner || account?.is_admin) && row.original.role !== "owner" ? (
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setTransfer(row.original)}
            >
              Transfer ownership
            </Button>
          ) : null,
      },
    ],
    [owner, account?.is_admin],
  );

  return (
    <ConsoleShell
      crumbs={[{ label: "Projects", href: "/projects" }, { label: project?.name ?? "Project" }]}
    >
      <PageHeader
        title={project?.name ?? "Project"}
        description={
          project
            ? `${project.repository} · Decisions are visible to every member.`
            : "Loading project…"
        }
      />
      {error ? (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}
      {project ? (
        <Tabs defaultValue="decisions">
          <TabsList>
            <TabsTrigger value="decisions">Memories</TabsTrigger>
            <TabsTrigger value="memories">Branches</TabsTrigger>
            <TabsTrigger value="members">Members</TabsTrigger>
          </TabsList>
          <TabsContent value="decisions">
            <DataTable
              label="memories"
              data={decisions}
              columns={columns}
              getRowId={(m) => m.id}
              filterPlaceholder="Filter memories"
              empty={
                <p className="p-6 text-sm text-muted-foreground">
                  No project memories yet. Notes and captures from this initialized repository are visible to every member.
                </p>
              }
            />
          </TabsContent>
          <TabsContent value="memories">
            <ProjectBranches branches={branches} />
          </TabsContent>
          <TabsContent value="members">
            <DataTable
              label="members"
              data={members}
              columns={memberColumns}
              getRowId={(m) => m.person_id}
              filterPlaceholder="Filter members"
              empty={<p>No members found.</p>}
            />
          </TabsContent>
        </Tabs>
      ) : null}
      <FormDialog
        open={action !== null}
        onOpenChange={(open) => !open && setAction(null)}
        title="Forget memory"
        description={
          <>
            {action ? deleteDecisionImpact(action.memory.id) : ""}
            <span className="mt-2 block font-mono text-xs break-all">{action?.memory.id}</span>
          </>
        }
        submitLabel="Forget memory"
        onSubmit={async () => {
          if (!action) return;
          await submit(
            () =>
              cairn("delete", {
                method: "POST",
                body: JSON.stringify({ id: action.memory.id }),
              }),
            "Memory forgotten.",
          );
        }}
      >
        <p className="text-sm text-muted-foreground">
          To correct a memory, ask the agent to save the corrected version; it supersedes this one.
        </p>
      </FormDialog>
      <FormDialog
        open={transfer !== null}
        onOpenChange={(open) => !open && setTransfer(null)}
        title="Transfer ownership"
        description={`Make ${transfer?.email ?? "this member"} the project owner. You remain a member; your private memories and branch records stay yours.`}
        submitLabel="Transfer ownership"
        onSubmit={async () => {
          if (transfer)
            await submit(
              () =>
                cairn(`${path}/ownership`, {
                  method: "POST",
                  body: JSON.stringify({ personId: transfer.person_id }),
                }),
              "Project ownership transferred.",
            );
        }}
      >
        <p className="text-sm text-muted-foreground">
          The new owner can manage every project decision and transfer ownership again.
        </p>
      </FormDialog>
      <Sheet open={detail !== null} onOpenChange={(open) => !open && setDetail(null)}>
        <SheetContent className="overflow-y-auto">
          <SheetHeader>
            <SheetTitle>{detail?.title ?? "Project memory"}</SheetTitle>
            <SheetDescription>{detail?.id}</SheetDescription>
          </SheetHeader>
          <div className="space-y-4 p-4">
            {detail ? <MemoryFeedback memory={detail} onUpdated={(updated) => {
              setDetail(updated);
              setDecisions((rows) => rows.map((row) => row.id === updated.id ? updated : row));
            }} /> : null}
            {detail ? Object.entries(detail).map(([field, value]) => (
              <div key={field}>
                <p className="text-xs font-medium text-muted-foreground">{field.replaceAll("_", " ")}</p>
                <p className="whitespace-pre-wrap break-words text-sm">{typeof value === "object" && value !== null ? JSON.stringify(value, null, 2) : String(value ?? "—")}</p>
              </div>
            )) : null}
          </div>
        </SheetContent>
      </Sheet>
    </ConsoleShell>
  );
}
