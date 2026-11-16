"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import type { ColumnDef } from "@tanstack/react-table";
import { MessageSquareTextIcon, MoreHorizontalIcon, UsersIcon } from "lucide-react";
import { toast } from "sonner";
import { Freshness, IdText, KindBadge, RoleBadge, ScopeBadge, SensitivityBadge } from "@/components/badges";
import { ConsoleShell } from "@/components/console-shell";
import { DataTable } from "@/components/data-table";
import { FormDialog } from "@/components/form-dialog";
import { PageHeader } from "@/components/page-header";
import { ProjectMemories } from "@/components/project-memories";
import { ProjectOwnerActions } from "@/components/project-owner-actions";
import { ProjectReports } from "@/components/project-reports";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { deleteDecisionImpact, revokeShareImpact } from "@/lib/action-copy";
import { cairn } from "@/lib/cairn";
import { memoryReadPath } from "@/lib/memory-path";
import { formatTime } from "@/lib/report";
import { SessionRequiredError } from "@/lib/session";
import { isAbsentTeam } from "@/lib/team";
import type { ListedProject, ProjectBranch, ProjectMemory, ProjectTeam } from "@/lib/types";

function field(value: string | number | null | undefined): string {
  if (value === null || value === undefined || value === "") {
    return "—";
  }
  return String(value);
}

type ConfirmAction = { kind: "delete" | "revoke"; memoryId: string };

export function ProjectDetail({ projectId }: { projectId: string }) {
  const router = useRouter();
  const [project, setProject] = useState<ListedProject | null>(null);
  const [memories, setMemories] = useState<ProjectMemory[]>([]);
  const [branches, setBranches] = useState<ProjectBranch[]>([]);
  const [team, setTeam] = useState<ProjectTeam | null>(null);
  const [teamError, setTeamError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [email, setEmail] = useState("");
  const [pending, setPending] = useState(false);
  const [sessionRequired, setSessionRequired] = useState(false);
  const [confirm, setConfirm] = useState<ConfirmAction | null>(null);
  const [dialogAction, setDialogAction] = useState<ConfirmAction | null>(null);
  const [detail, setDetail] = useState<ProjectMemory | null>(null);
  const [verifying, setVerifying] = useState<ProjectMemory | null>(null);
  const confirming = useRef(false);
  const loadGeneration = useRef(0);

  function beginLoad(): number {
    loadGeneration.current += 1;
    return loadGeneration.current;
  }

  function isCurrent(generation: number): boolean {
    return generation === loadGeneration.current;
  }

  useResetOnHide(() => {
    beginLoad();
    confirming.current = false;
    setProject(null);
    setMemories([]);
    setBranches([]);
    setTeam(null);
    setTeamError(null);
    setError(null);
    setActionError(null);
    setEmail("");
    setPending(false);
    setSessionRequired(false);
    setConfirm(null);
    setDialogAction(null);
    setDetail(null);
    setVerifying(null);
  });

  function openConfirm(next: ConfirmAction) {
    setActionError(null);
    setDialogAction(next);
    setConfirm(next);
  }

  const requireSignIn = useCallback(() => {
    setSessionRequired(true);
    setProject(null);
    setMemories([]);
    setBranches([]);
    setTeam(null);
    setTeamError(null);
    setError(null);
    setActionError(null);
    router.replace("/");
  }, [router]);

  const applyProject = useCallback(
    (result: {
      current: ListedProject;
      decisions: ProjectMemory[];
      nextBranches: ProjectBranch[];
      nextTeam: ProjectTeam | null;
      nextTeamError: string | null;
    }) => {
      setProject(result.current);
      setMemories(result.decisions);
      setBranches(result.nextBranches);
      setTeam(result.nextTeam);
      setTeamError(result.nextTeamError);
      setError(null);
    },
    [],
  );

  const fetchProject = useCallback(async () => {
    const listed = await cairn<ListedProject[]>("projects");
    const current = listed.find((item) => item.project_id === projectId);
    if (!current) {
      throw new Error("Project is not on this account.");
    }
    const decisions = await cairn<ProjectMemory[]>(memoryReadPath(projectId, current.role));
    const nextBranches =
      current.role === "owner"
        ? await cairn<ProjectBranch[]>(`projects/${encodeURIComponent(projectId)}/branches`)
        : [];
    let nextTeam: ProjectTeam | null = null;
    let nextTeamError: string | null = null;
    try {
      nextTeam = await cairn<ProjectTeam>(
        `projects/${encodeURIComponent(projectId)}/team`,
      );
    } catch (cause) {
      if (cause instanceof SessionRequiredError) {
        throw cause;
      }
      const message = cause instanceof Error ? cause.message : "Team request failed";
      if (!isAbsentTeam(message)) {
        nextTeamError = message;
      }
    }
    return { current, decisions, nextBranches, nextTeam, nextTeamError };
  }, [projectId]);

  useEffect(() => {
    let cancelled = false;
    const generation = beginLoad();
    fetchProject()
      .then((result) => {
        if (cancelled || !isCurrent(generation)) {
          return;
        }
        applyProject(result);
      })
      .catch((cause: unknown) => {
        if (cancelled || !isCurrent(generation)) {
          return;
        }
        if (cause instanceof SessionRequiredError) {
          requireSignIn();
          return;
        }
        setError(cause instanceof Error ? cause.message : "Could not load the project");
      });
    return () => {
      cancelled = true;
    };
  }, [applyProject, fetchProject, projectId, requireSignIn]);

  const owner = project?.role === "owner";

  async function run(action: () => Promise<unknown>, success?: string): Promise<boolean> {
    const generation = beginLoad();
    setPending(true);
    setActionError(null);
    try {
      await action();
      const result = await fetchProject();
      if (isCurrent(generation)) {
        applyProject(result);
        if (success) {
          toast.success(success);
        }
        return true;
      }
    } catch (cause) {
      if (!isCurrent(generation)) {
        return false;
      }
      if (cause instanceof SessionRequiredError) {
        requireSignIn();
        return false;
      }
      const message = cause instanceof Error ? cause.message : "Request failed";
      setActionError(message);
      toast.error(message);
    } finally {
      if (isCurrent(generation)) {
        setPending(false);
      }
    }
    return false;
  }

  /** A write from a dialog: errors throw into the dialog, then the project reloads. */
  async function submit(action: () => Promise<unknown>, success: string) {
    await action();
    const generation = beginLoad();
    try {
      const result = await fetchProject();
      if (isCurrent(generation)) {
        applyProject(result);
      }
    } catch (cause) {
      if (!isCurrent(generation)) {
        return;
      }
      if (cause instanceof SessionRequiredError) {
        requireSignIn();
        return;
      }
      setError("The change was saved, but the project could not be reloaded. Reload the page.");
    }
    toast.success(success);
  }

  async function confirmAction() {
    if (!confirm || confirming.current) {
      return;
    }
    if (confirm.kind === "revoke" && email.trim() === "") {
      return;
    }
    const action = confirm;
    const generation = beginLoad();
    confirming.current = true;
    setPending(true);
    setActionError(null);
    try {
      if (action.kind === "delete") {
        await cairn("delete", {
          method: "POST",
          body: JSON.stringify({ id: action.memoryId }),
        });
        setMemories((current) =>
          current.filter((item) => item.memory_id !== action.memoryId),
        );
      } else {
        await cairn(
          `projects/${encodeURIComponent(projectId)}/shares/${encodeURIComponent(action.memoryId)}/revoke`,
          {
            method: "POST",
            body: JSON.stringify({ email }),
          },
        );
      }
    } catch (cause) {
      if (isCurrent(generation)) {
        if (cause instanceof SessionRequiredError) {
          requireSignIn();
        } else {
          setActionError(cause instanceof Error ? cause.message : "Request failed");
        }
      }
      if (generation === loadGeneration.current) {
        confirming.current = false;
        setPending(false);
      }
      return;
    }
    try {
      const result = await fetchProject();
      if (isCurrent(generation)) {
        applyProject(result);
        setConfirm(null);
        toast.success(action.kind === "delete" ? "Decision deleted." : "Share revoked.");
      }
    } catch (cause) {
      if (!isCurrent(generation)) {
        return;
      }
      if (cause instanceof SessionRequiredError) {
        requireSignIn();
        return;
      }
      setConfirm(null);
      setError(
        action.kind === "delete"
          ? "The decision was hidden, but the project could not be reloaded. Do not delete it again."
          : "The share was revoked, but the project could not be reloaded. Do not revoke it again.",
      );
    } finally {
      if (generation === loadGeneration.current) {
        confirming.current = false;
        setPending(false);
      }
    }
  }

  function shareDecision(memoryId: string) {
    if (email.trim() === "") {
      toast.error("Enter a member email under “Share with” first.");
      document.getElementById("member-email")?.focus();
      return;
    }
    void run(
      () =>
        cairn(`projects/${encodeURIComponent(projectId)}/shares`, {
          method: "POST",
          body: JSON.stringify({ memory_id: memoryId, email }),
        }),
      "Decision shared.",
    );
  }

  const columns = useMemo<ColumnDef<ProjectMemory>[]>(() => {
    const cols: ColumnDef<ProjectMemory>[] = [
      {
        id: "decision",
        header: "Decision",
        accessorFn: (memory) => `${memory.memory_id} ${memory.content}`,
        enableSorting: false,
        cell: ({ row }) => (
          <div className="flex min-w-0 flex-col gap-1.5">
            <button
              type="button"
              onClick={() => setDetail(row.original)}
              aria-haspopup="dialog"
              className="line-clamp-2 w-full rounded-sm text-left text-sm [overflow-wrap:anywhere] underline-offset-4 outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
            >
              {row.original.content}
            </button>
            <IdText id={row.original.memory_id} />
          </div>
        ),
      },
      {
        id: "scope",
        header: "Scope",
        accessorFn: (memory) => memory.scope ?? "",
        cell: ({ row }) => <ScopeBadge scope={row.original.scope} />,
        meta: { className: "hidden md:table-cell" },
      },
      {
        id: "sensitivity",
        header: "Sensitivity",
        accessorFn: (memory) => memory.lifecycle?.sensitivity ?? "",
        cell: ({ row }) => <SensitivityBadge sensitivity={row.original.lifecycle?.sensitivity} />,
        meta: { className: "hidden lg:table-cell" },
      },
      {
        id: "freshness",
        header: "Freshness",
        accessorFn: (memory) => memory.lifecycle?.freshness ?? -1,
        cell: ({ row }) => <Freshness value={row.original.lifecycle?.freshness} />,
        meta: { className: "hidden lg:table-cell" },
      },
      {
        id: "verified",
        header: "Verified at",
        accessorFn: (memory) => memory.verified_commit ?? "",
        cell: ({ row }) =>
          row.original.verified_commit ? (
            <span className="font-mono text-xs break-all">{row.original.verified_commit}</span>
          ) : (
            <span className="text-muted-foreground">—</span>
          ),
        meta: { className: "hidden xl:table-cell" },
      },
      {
        id: "created",
        header: "Created",
        accessorFn: (memory) => Number(memory.created_at ?? 0),
        cell: ({ row }) => (
          <span className="text-xs whitespace-nowrap text-muted-foreground">
            {formatTime(row.original.created_at)}
          </span>
        ),
        meta: { className: "hidden md:table-cell" },
      },
    ];
    if (owner) {
      cols.push({
        id: "actions",
        header: () => <span className="sr-only">Actions</span>,
        enableSorting: false,
        cell: ({ row }) => (
          <div className="flex justify-end">
            <DropdownMenu>
              <DropdownMenuTrigger
                render={
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    disabled={pending}
                    aria-label={`Actions for ${row.original.memory_id}`}
                  />
                }
              >
                <MoreHorizontalIcon />
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-52">
                <DropdownMenuItem onClick={() => setDetail(row.original)}>View details</DropdownMenuItem>
                <DropdownMenuItem onClick={() => setVerifying(row.original)}>
                  Verify against commit
                </DropdownMenuItem>
                <DropdownMenuItem onClick={() => shareDecision(row.original.memory_id)}>
                  Share decision
                </DropdownMenuItem>
                <DropdownMenuItem
                  onClick={() => openConfirm({ kind: "revoke", memoryId: row.original.memory_id })}
                >
                  Revoke share
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  variant="destructive"
                  onClick={() => openConfirm({ kind: "delete", memoryId: row.original.memory_id })}
                >
                  Delete memory
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        ),
        meta: { className: "w-10" },
      });
    }
    return cols;
    // shareDecision and openConfirm close over `email` and the setters; rebuild when those change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [owner, pending, email, projectId]);

  if (sessionRequired) {
    return (
      <main className="mx-auto flex min-h-svh w-full max-w-sm flex-1 items-center px-6">
        <p className="text-sm text-muted-foreground">Sign in is required.</p>
      </main>
    );
  }

  const title = project?.name ?? (error ? "Project unavailable" : "Project");

  return (
    <ConsoleShell crumbs={[{ label: "Projects", href: "/projects" }, { label: title }]}>
      <PageHeader
        title={title}
        description={
          project ? (
            <span className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
              <span className="font-mono text-xs break-all">{project.repository}</span>
              <RoleBadge role={project.role} />
              <IdText id={project.project_id} />
            </span>
          ) : undefined
        }
      />
      {!project && !error ? (
        <div role="status" className="flex flex-col gap-3">
          <span className="sr-only">Loading project…</span>
          <Skeleton className="h-8 w-80 max-w-full" />
          <Skeleton className="h-64 w-full" />
        </div>
      ) : null}
      {error ? (
        <Alert variant="destructive">
          <AlertDescription className="text-destructive">{error}</AlertDescription>
        </Alert>
      ) : null}

      {project ? (
        <Tabs defaultValue="decisions" className="min-w-0 gap-5">
          <TabsList variant="line" className="w-full justify-start overflow-x-auto border-b">
            <TabsTrigger value="decisions" className="flex-none px-3">
              Decisions
              <Badge variant="secondary">{memories.length}</Badge>
            </TabsTrigger>
            {owner ? (
              <TabsTrigger value="memories" className="flex-none px-3">
                Memories
              </TabsTrigger>
            ) : null}
            {owner ? (
              <TabsTrigger value="reports" className="flex-none px-3">
                Reports
              </TabsTrigger>
            ) : null}
            <TabsTrigger value="team" className="flex-none px-3">
              Team &amp; sharing
            </TabsTrigger>
            {owner ? (
              <TabsTrigger value="controls" className="flex-none px-3">
                Controls
              </TabsTrigger>
            ) : null}
          </TabsList>

          <TabsContent value="decisions" className="flex min-w-0 flex-col gap-4">
            <p className="max-w-prose text-sm text-muted-foreground">
              Settled project knowledge promoted from a memory. Shared with members you choose and
              verifiable against a commit. Promoting keeps your personal memory.
            </p>
            <DataTable
              label="decisions"
              data={memories}
              columns={columns}
              getRowId={(memory) => memory.memory_id}
              filterPlaceholder="Filter decisions"
              toolbar={
                owner ? (
                  <Field className="sm:ml-auto sm:w-80">
                    <FieldLabel htmlFor="member-email" className="sr-only">
                      Share with
                    </FieldLabel>
                    <Input
                      id="member-email"
                      type="email"
                      list="member-emails"
                      value={email}
                      onChange={(event) => setEmail(event.target.value)}
                      autoComplete="off"
                      placeholder="Share with: member email"
                      title="The member that Share decision and Revoke share act on"
                    />
                    <datalist id="member-emails">
                      {team?.members.map((member) => (
                        <option key={member.person_id} value={member.email} />
                      ))}
                    </datalist>
                  </Field>
                ) : null
              }
              initialSort={[{ id: "created", desc: true }]}
              empty={
                <Empty className="border-0 p-6">
                  <EmptyHeader>
                    <EmptyMedia variant="icon">
                      <MessageSquareTextIcon />
                    </EmptyMedia>
                    <EmptyTitle>{owner ? "No decisions" : "No shared decisions"}</EmptyTitle>
                    <EmptyDescription>
                      {owner
                        ? "Promote a memory from the Memories tab to record the first decision."
                        : "Decisions the owner shares with you appear here."}
                    </EmptyDescription>
                  </EmptyHeader>
                </Empty>
              }
            />
          </TabsContent>

          {owner ? (
            <TabsContent value="memories" className="min-w-0">
              <ProjectMemories
                projectId={projectId}
                repository={project.repository}
                submit={submit}
                onSessionRequired={requireSignIn}
              />
            </TabsContent>
          ) : null}

          {owner ? (
            <TabsContent value="reports" className="min-w-0">
              <ProjectReports
                projectId={projectId}
                memberEmails={team?.members.map((member) => member.email) ?? []}
                decisions={memories}
                onSessionRequired={requireSignIn}
              />
            </TabsContent>
          ) : null}

          <TabsContent value="team" className="flex min-w-0 flex-col gap-4">
            {teamError ? (
              <Alert variant="destructive">
                <AlertDescription className="text-destructive">{teamError}</AlertDescription>
              </Alert>
            ) : null}
            <p className="max-w-prose text-sm text-muted-foreground">
              Members join automatically when they work in a checkout that contains this
              project&apos;s <code className="font-mono text-xs text-foreground">.cairn/project.json</code>{" "}
              (created by <code className="font-mono text-xs text-foreground">cairn init</code>).
              {owner ? " Share a decision with a member from the Decisions tab." : ""}
            </p>
            <Card className="min-w-0 max-w-xl">
              <CardHeader>
                <CardTitle className="break-words">
                  <h2>Members</h2>
                </CardTitle>
                <CardDescription>
                  {team ? `${team.members.length} ${team.members.length === 1 ? "member" : "members"}` : "People who work on this project."}
                </CardDescription>
              </CardHeader>
              <CardContent>
                {team && team.members.length > 0 ? (
                  <ul className="flex flex-col gap-2 text-sm">
                    {team.members.map((member) => (
                      <li key={member.person_id} className="flex min-w-0 items-center gap-2.5">
                        <Avatar className="size-6 rounded-md">
                          <AvatarFallback className="rounded-md text-xs">
                            {member.email.slice(0, 1).toUpperCase()}
                          </AvatarFallback>
                        </Avatar>
                        <span className="break-all">{member.email}</span>
                      </li>
                    ))}
                  </ul>
                ) : !teamError ? (
                  <Empty className="border border-dashed p-6">
                    <EmptyHeader>
                      <EmptyMedia variant="icon">
                        <UsersIcon />
                      </EmptyMedia>
                      <EmptyTitle>No members yet</EmptyTitle>
                      <EmptyDescription>
                        Members appear here after they work in a checkout of this project.
                      </EmptyDescription>
                    </EmptyHeader>
                  </Empty>
                ) : null}
              </CardContent>
            </Card>
          </TabsContent>

          {owner ? (
            <TabsContent value="controls" className="min-w-0">
              <ProjectOwnerActions
                projectId={projectId}
                sharePolicy={project.share_policy}
                branches={branches}
                pending={pending}
                run={run}
                onSessionRequired={requireSignIn}
              />
            </TabsContent>
          ) : null}
        </Tabs>
      ) : null}

      <Sheet open={detail !== null} onOpenChange={(open) => !open && setDetail(null)}>
        <SheetContent className="w-full overflow-y-auto sm:max-w-lg">
          {detail ? (
            <>
              <SheetHeader>
                <SheetTitle>Decision</SheetTitle>
                <SheetDescription className="break-all">{detail.memory_id}</SheetDescription>
              </SheetHeader>
              <div className="flex min-w-0 flex-col gap-5 px-4 pb-6">
                <p className="text-sm whitespace-pre-wrap [overflow-wrap:anywhere]">{detail.content}</p>
                <dl className="grid min-w-0 grid-cols-[max-content_minmax(0,1fr)] items-center gap-x-4 gap-y-2.5 text-sm">
                  <dt className="text-muted-foreground">Kind</dt>
                  <dd>
                    <KindBadge kind={detail.kind} />
                  </dd>
                  <dt className="text-muted-foreground">Scope</dt>
                  <dd>
                    <ScopeBadge scope={detail.scope} />
                  </dd>
                  <dt className="text-muted-foreground">Sensitivity</dt>
                  <dd>
                    <SensitivityBadge sensitivity={detail.lifecycle?.sensitivity} />
                  </dd>
                  <dt className="text-muted-foreground">Freshness</dt>
                  <dd>
                    <Freshness value={detail.lifecycle?.freshness} />
                  </dd>
                  <dt className="text-muted-foreground">Verified commit</dt>
                  <dd className="min-w-0 font-mono text-xs break-all">{field(detail.verified_commit)}</dd>
                  <dt className="text-muted-foreground">Supersedes</dt>
                  <dd className="min-w-0 font-mono text-xs break-all">{field(detail.supersedes)}</dd>
                  {detail.share_reason ? (
                    <>
                      <dt className="text-muted-foreground">Share reason</dt>
                      <dd className="min-w-0 break-words">{detail.share_reason}</dd>
                    </>
                  ) : null}
                  <dt className="text-muted-foreground">Created</dt>
                  <dd>{formatTime(detail.created_at)}</dd>
                </dl>
              </div>
            </>
          ) : null}
        </SheetContent>
      </Sheet>

      <FormDialog
        open={verifying !== null}
        onOpenChange={(open) => !open && setVerifying(null)}
        title="Verify against commit"
        description={
          <>
            Record the commit where you checked that this decision still holds.
            <span className="mt-2 line-clamp-3 block rounded-md bg-muted px-2.5 py-2 text-foreground [overflow-wrap:anywhere]">
              {verifying?.content}
            </span>
          </>
        }
        submitLabel="Verify decision"
        onSubmit={async (data) => {
          if (!verifying) {
            return;
          }
          const commit = String(data.get("commit")).trim();
          if (!commit) {
            throw new Error("Enter the commit you checked the decision against.");
          }
          await submit(
            () =>
              cairn("projects/verify", {
                method: "POST",
                body: JSON.stringify({
                  project_id: projectId,
                  memory_id: verifying.memory_id,
                  commit,
                }),
              }),
            "Decision verified.",
          );
        }}
      >
        <Field>
          <FieldLabel htmlFor="verify-commit">Commit</FieldLabel>
          <Input id="verify-commit" name="commit" className="font-mono" autoComplete="off" required />
        </Field>
      </FormDialog>

      <AlertDialog
        open={confirm !== null}
        onOpenChange={(open) => {
          if (pending || open) {
            return;
          }
          setConfirm(null);
          setActionError(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {dialogAction?.kind === "revoke" ? "Revoke this share?" : "Delete this decision?"}
            </AlertDialogTitle>
            <AlertDialogDescription className="text-left [overflow-wrap:anywhere]">
              {dialogAction?.kind === "revoke"
                ? revokeShareImpact(dialogAction.memoryId, email)
                : dialogAction
                  ? deleteDecisionImpact(dialogAction.memoryId)
                  : ""}
            </AlertDialogDescription>
          </AlertDialogHeader>
          {confirm?.kind === "revoke" ? (
            <Field>
              <FieldLabel htmlFor="revoke-member-email">Member email</FieldLabel>
              <Input
                id="revoke-member-email"
                type="email"
                value={email}
                onChange={(event) => setEmail(event.target.value)}
                autoComplete="off"
              />
            </Field>
          ) : null}
          {actionError ? (
            <Alert variant="destructive">
              <AlertDescription className="text-destructive">{actionError}</AlertDescription>
            </Alert>
          ) : null}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={
                pending || (confirm?.kind === "revoke" && email.trim() === "")
              }
              onClick={() => void confirmAction()}
            >
              {pending ? (
                <>
                  <Spinner aria-hidden />
                  Working…
                </>
              ) : dialogAction?.kind === "revoke" ? (
                "Revoke share"
              ) : (
                "Delete memory"
              )}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </ConsoleShell>
  );
}
