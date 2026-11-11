"use client";

import { useCallback, useEffect, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cairn } from "@/lib/cairn";
import { memoryReadPath } from "@/lib/memory-path";
import type { ListedProject, ProjectMemory, ProjectTeam } from "@/lib/types";

function field(value: string | number | null | undefined): string {
  if (value === null || value === undefined || value === "") {
    return "—";
  }
  return String(value);
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
      nextTeamError = cause instanceof Error ? cause.message : "Team request failed";
    }
    return { current, decisions, nextTeam, nextTeamError };
  }, [projectId]);

  useEffect(() => {
    let cancelled = false;
    fetchProject()
      .then((result) => {
        if (!cancelled) {
          applyProject(result);
        }
      })
      .catch((cause: unknown) => {
        if (!cancelled) {
          setError(cause instanceof Error ? cause.message : "Could not load the project");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [applyProject, fetchProject]);

  const owner = project?.role === "owner";

  async function run(action: () => Promise<unknown>) {
    setPending(true);
    setActionError(null);
    try {
      await action();
      applyProject(await fetchProject());
    } catch (cause) {
      setActionError(cause instanceof Error ? cause.message : "Request failed");
    } finally {
      setPending(false);
    }
  }

  async function logout() {
    try {
      await cairn("auth/logout", { method: "POST" });
    } catch (cause) {
      setActionError(cause instanceof Error ? cause.message : "Logout failed");
      return;
    }
    router.push("/");
  }

  return (
    <main className="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-6 px-4 py-10">
      <div className="flex items-center justify-between gap-4">
        <Link href="/projects" className="text-sm text-muted-foreground">
          Projects
        </Link>
        <Button type="button" variant="outline" onClick={() => void logout()}>
          Log out
        </Button>
      </div>
      <header className="flex flex-col gap-1">
        <h1 className="text-xl font-medium">{project?.name ?? "Project"}</h1>
        {project ? (
          <p className="text-sm text-muted-foreground">
            {project.repository} · {project.role} · {project.project_id}
          </p>
        ) : null}
      </header>
      {!project && !error ? (
        <p className="text-sm text-muted-foreground">Loading project…</p>
      ) : null}
      {error ? (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      ) : null}
      {actionError ? (
        <p role="alert" className="text-sm text-destructive">
          {actionError}
        </p>
      ) : null}

      <section className="flex flex-col gap-3">
        <h2 className="text-base font-medium">Decisions</h2>
        {project && memories.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            {owner ? "No decisions." : "No shared decisions."}
          </p>
        ) : null}
        <ul className="flex flex-col gap-3">
          {memories.map((memory) => (
            <li key={memory.memory_id}>
              <Card>
                <CardHeader>
                  <CardTitle>{memory.memory_id}</CardTitle>
                </CardHeader>
                <CardContent className="flex flex-col gap-3">
                  <p className="whitespace-pre-wrap text-sm">{memory.content}</p>
                  <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1 text-sm">
                    <dt>scope</dt>
                    <dd>{field(memory.scope)}</dd>
                    <dt>verified_commit</dt>
                    <dd>{field(memory.verified_commit)}</dd>
                    <dt>lifecycle.freshness</dt>
                    <dd>{field(memory.lifecycle?.freshness)}</dd>
                    <dt>supersedes</dt>
                    <dd>{field(memory.supersedes)}</dd>
                    {memory.share_reason ? (
                      <>
                        <dt>share_reason</dt>
                        <dd>{memory.share_reason}</dd>
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
                        onClick={() =>
                          void run(() =>
                            cairn(
                              `projects/${encodeURIComponent(projectId)}/shares/${encodeURIComponent(memory.memory_id)}/revoke`,
                              {
                                method: "POST",
                                body: JSON.stringify({ email }),
                              },
                            ),
                          )
                        }
                      >
                        Revoke share
                      </Button>
                      <Button
                        type="button"
                        variant="destructive"
                        disabled={pending}
                        onClick={() =>
                          void run(() =>
                            cairn("delete", {
                              method: "POST",
                              body: JSON.stringify({ id: memory.memory_id }),
                            }),
                          )
                        }
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

      <section className="flex flex-col gap-3">
        <h2 className="text-base font-medium">Team</h2>
        {teamError ? (
          <p role="alert" className="text-sm text-destructive">
            {teamError}
          </p>
        ) : null}
        {team ? (
          <Card>
            <CardHeader>
              <CardTitle>{team.name}</CardTitle>
              <CardDescription>{team.project_id}</CardDescription>
            </CardHeader>
            <CardContent>
              {team.members.length === 0 ? (
                <p className="text-sm text-muted-foreground">No team members.</p>
              ) : (
                <ul className="text-sm">
                  {team.members.map((member) => (
                    <li key={member.person_id}>{member.email}</li>
                  ))}
                </ul>
              )}
            </CardContent>
          </Card>
        ) : null}
      </section>

      {owner ? (
        <section className="flex flex-col gap-4">
          <h2 className="text-base font-medium">Owner actions</h2>
          <div className="flex flex-col gap-2">
            <Label htmlFor="member-email">Member email</Label>
            <Input
              id="member-email"
              type="email"
              value={email}
              onChange={(event) => setEmail(event.target.value)}
              autoComplete="off"
            />
          </div>
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
              <Label htmlFor="team-name">Team name</Label>
              <Input
                id="team-name"
                value={teamName}
                onChange={(event) => setTeamName(event.target.value)}
                required
              />
              <Button type="submit" disabled={pending}>
                Create team
              </Button>
            </form>
          )}
        </section>
      ) : null}
    </main>
  );
}
