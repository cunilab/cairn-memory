"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import { ConsoleShell } from "@/components/console-shell";
import { ProjectReports } from "@/components/project-reports";
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
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { deleteDecisionImpact, revokeShareImpact } from "@/lib/action-copy";
import { cairn } from "@/lib/cairn";
import { memoryReadPath } from "@/lib/memory-path";
import { SessionRequiredError } from "@/lib/session";
import { isAbsentTeam } from "@/lib/team";
import type { ListedProject, ProjectMemory, ProjectTeam } from "@/lib/types";

function field(value: string | number | null | undefined): string {
  if (value === null || value === undefined || value === "") {
    return "—";
  }
  return String(value);
}

type ConfirmAction = { kind: "delete" | "revoke"; memoryId: string };

function roleLabel(role: string): string {
  if (role === "owner") {
    return "Owner";
  }
  if (role === "member") {
    return "Member";
  }
  return role;
}

export function ProjectDetail({ projectId }: { projectId: string }) {
  const router = useRouter();
  const [project, setProject] = useState<ListedProject | null>(null);
  const [memories, setMemories] = useState<ProjectMemory[]>([]);
  const [team, setTeam] = useState<ProjectTeam | null>(null);
  const [teamError, setTeamError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [email, setEmail] = useState("");
  const [teamName, setTeamName] = useState("");
  const [pending, setPending] = useState(false);
  const [sessionRequired, setSessionRequired] = useState(false);
  const [confirm, setConfirm] = useState<ConfirmAction | null>(null);
  const [dialogAction, setDialogAction] = useState<ConfirmAction | null>(null);
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
    setTeam(null);
    setTeamError(null);
    setError(null);
    setActionError(null);
    setEmail("");
    setTeamName("");
    setPending(false);
    setSessionRequired(false);
    setConfirm(null);
    setDialogAction(null);
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
      nextTeam: ProjectTeam | null;
      nextTeamError: string | null;
    }) => {
      setProject(result.current);
      setMemories(result.decisions);
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
    return { current, decisions, nextTeam, nextTeamError };
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

  async function run(action: () => Promise<unknown>) {
    const generation = beginLoad();
    setPending(true);
    setActionError(null);
    try {
      await action();
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
      setActionError(cause instanceof Error ? cause.message : "Request failed");
    } finally {
      if (isCurrent(generation)) {
        setPending(false);
      }
    }
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

  async function logout() {
    try {
      await cairn("auth/logout", { method: "POST" });
    } catch (cause) {
      if (cause instanceof SessionRequiredError) {
        requireSignIn();
        return;
      }
      setActionError(cause instanceof Error ? cause.message : "Logout failed");
      return;
    }
    router.replace("/");
  }

  if (sessionRequired) {
    return (
      <main className="mx-auto flex min-h-svh w-full max-w-sm flex-1 items-center px-6">
        <p className="text-sm text-muted-foreground">Sign in is required.</p>
      </main>
    );
  }

  return (
    <ConsoleShell
      title={project?.name ?? (error ? "Project unavailable" : "Project")}
      action={
        <Button type="button" variant="outline" onClick={() => void logout()}>
          Log out
        </Button>
      }
    >
      <div className="flex min-w-0 flex-col gap-6">
      {project ? (
        <p className="break-all text-sm text-muted-foreground">
          {project.repository} · {roleLabel(project.role)} · {project.project_id}
        </p>
      ) : null}
      {!project && !error ? (
        <p role="status" className="text-sm text-muted-foreground">
          Loading project…
        </p>
      ) : null}
      {error ? (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      ) : null}
      {actionError ? (
        // The owner form sits far below the buttons, so bring the failure into view.
        <p
          role="alert"
          ref={(element) => element?.scrollIntoView({ block: "nearest" })}
          className="text-sm text-destructive"
        >
          {actionError}
        </p>
      ) : null}

      {project ? (
      <>
      <section className="flex min-w-0 flex-col gap-3">
        <h2 className="text-base font-medium">Decisions</h2>
        {project && memories.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            {owner ? "No decisions." : "No shared decisions."}
          </p>
        ) : null}
        <ul className="flex flex-col gap-3">
          {memories.map((memory) => (
            <li key={memory.memory_id} className="min-w-0">
              <Card className="min-w-0">
                <CardHeader>
                  <CardTitle className="break-all">{memory.memory_id}</CardTitle>
                </CardHeader>
                <CardContent className="flex min-w-0 flex-col gap-3">
                  <p className="whitespace-pre-wrap text-sm [overflow-wrap:anywhere]">
                    {memory.content}
                  </p>
                  <dl className="grid min-w-0 grid-cols-[max-content_minmax(0,1fr)] gap-x-3 gap-y-1 text-sm">
                    <dt className="text-muted-foreground">Scope</dt>
                    <dd className="min-w-0 break-all">{field(memory.scope)}</dd>
                    <dt className="text-muted-foreground">Verified commit</dt>
                    <dd className="min-w-0 break-all">{field(memory.verified_commit)}</dd>
                    <dt className="text-muted-foreground">Freshness</dt>
                    <dd className="min-w-0 break-all">{field(memory.lifecycle?.freshness)}</dd>
                    <dt className="text-muted-foreground">Supersedes</dt>
                    <dd className="min-w-0 break-all">{field(memory.supersedes)}</dd>
                    {memory.share_reason ? (
                      <>
                        <dt className="text-muted-foreground">Share reason</dt>
                        <dd className="min-w-0 break-all">{memory.share_reason}</dd>
                      </>
                    ) : null}
                  </dl>
                  {owner ? (
                    <div className="flex flex-wrap gap-2">
                      <Button
                        type="button"
                        variant="outline"
                        disabled={pending}
                        onClick={() =>
                          void run(() =>
                            cairn(`projects/${encodeURIComponent(projectId)}/shares`, {
                              method: "POST",
                              body: JSON.stringify({
                                memory_id: memory.memory_id,
                                email,
                              }),
                            }),
                          )
                        }
                      >
                        Share decision
                      </Button>
                      <Button
                        type="button"
                        variant="outline"
                        disabled={pending}
                        onClick={() => {
                          openConfirm({ kind: "revoke", memoryId: memory.memory_id });
                        }}
                      >
                        Revoke share
                      </Button>
                      <Button
                        type="button"
                        variant="destructive"
                        disabled={pending}
                        onClick={() => {
                          openConfirm({ kind: "delete", memoryId: memory.memory_id });
                        }}
                      >
                        Delete memory
                      </Button>
                    </div>
                  ) : null}
                </CardContent>
              </Card>
            </li>
          ))}
        </ul>
      </section>

      <section className="flex min-w-0 flex-col gap-3">
        <h2 className="text-base font-medium">Team</h2>
        {teamError ? (
          <p role="alert" className="text-sm text-destructive">
            {teamError}
          </p>
        ) : null}
        {team ? (
          <Card className="min-w-0">
            <CardHeader>
              <CardTitle className="break-words">{team.name}</CardTitle>
              <CardDescription className="break-all">{team.project_id}</CardDescription>
            </CardHeader>
            <CardContent>
              {team.members.length === 0 ? (
                <p className="text-sm text-muted-foreground">No team members.</p>
              ) : (
                <ul className="text-sm">
                  {team.members.map((member) => (
                    <li key={member.person_id} className="break-all">
                      {member.email}
                    </li>
                  ))}
                </ul>
              )}
            </CardContent>
          </Card>
        ) : null}
        {!team && !teamError ? (
          <p className="text-sm text-muted-foreground">
            {owner
              ? "No team yet. Create one with the form below."
              : "This project has no team."}
          </p>
        ) : null}
      </section>

      {owner ? (
        <ProjectReports
          projectId={projectId}
          memberEmails={team?.members.map((member) => member.email) ?? []}
          decisions={memories}
          onSessionRequired={requireSignIn}
        />
      ) : null}

      {owner ? (
        <section className="flex flex-col gap-4">
          <h2 className="text-base font-medium">Owner actions</h2>
          <FieldGroup>
          <Field>
            <FieldLabel htmlFor="member-email">Member email</FieldLabel>
            <Input
              id="member-email"
              type="email"
              value={email}
              onChange={(event) => setEmail(event.target.value)}
              autoComplete="off"
            />
          </Field>
          <div className="flex flex-wrap gap-2">
            <Button
              type="button"
              disabled={pending}
              onClick={() =>
                void run(() =>
                  cairn(`projects/${encodeURIComponent(projectId)}/members`, {
                    method: "POST",
                    body: JSON.stringify({ email }),
                  }),
                )
              }
            >
              Invite member
            </Button>
            <Button
              type="button"
              variant="outline"
              disabled={pending}
              onClick={() =>
                void run(() =>
                  cairn(`projects/${encodeURIComponent(projectId)}/team/members`, {
                    method: "POST",
                    body: JSON.stringify({ email }),
                  }),
                )
              }
            >
              Add member
            </Button>
            <Button
              type="button"
              variant="outline"
              disabled={pending}
              onClick={() =>
                void run(() =>
                  cairn(`projects/${encodeURIComponent(projectId)}/team/members/remove`, {
                    method: "POST",
                    body: JSON.stringify({ email }),
                  }),
                )
              }
            >
              Remove member
            </Button>
          </div>
          {team ? null : (
            <form
              className="flex flex-col gap-2"
              onSubmit={(event) => {
                event.preventDefault();
                void run(() =>
                  cairn(`projects/${encodeURIComponent(projectId)}/team`, {
                    method: "POST",
                    body: JSON.stringify({ name: teamName }),
                  }),
                );
              }}
            >
              <Field>
                <FieldLabel htmlFor="team-name">Team name</FieldLabel>
                <Input
                  id="team-name"
                  value={teamName}
                  onChange={(event) => setTeamName(event.target.value)}
                  required
                />
              </Field>
              <Button type="submit" disabled={pending}>
                Create team
              </Button>
            </form>
          )}
          </FieldGroup>
        </section>
      ) : null}
      </>
      ) : null}
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
          {actionError ? (
            <p role="alert" className="text-sm text-destructive">
              {actionError}
            </p>
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
              {pending
                ? "Working…"
                : dialogAction?.kind === "revoke"
                  ? "Revoke share"
                  : "Delete memory"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      </div>
    </ConsoleShell>
  );
}
