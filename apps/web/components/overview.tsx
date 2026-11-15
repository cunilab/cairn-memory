"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { BarList } from "@/components/bar-list";
import { ConsoleShell } from "@/components/console-shell";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { cairn } from "@/lib/cairn";
import { memoryReadPath } from "@/lib/memory-path";
import { formatTime, newestFirst } from "@/lib/report";
import { SessionRequiredError } from "@/lib/session";
import type { LifecycleSummary, ListedProject, ProjectMemory } from "@/lib/types";

type Decision = ProjectMemory & { projectId: string; projectName: string };

export function Overview() {
  const router = useRouter();
  const [projects, setProjects] = useState<ListedProject[]>([]);
  const [summary, setSummary] = useState<LifecycleSummary | null>(null);
  const [decisions, setDecisions] = useState<Decision[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [sessionRequired, setSessionRequired] = useState(false);

  useResetOnHide(() => {
    setProjects([]);
    setSummary(null);
    setDecisions([]);
    setError(null);
    setLoading(true);
    setLoadFailed(false);
    setSessionRequired(false);
  });

  useEffect(() => {
    let cancelled = false;
    async function load() {
      const listed = await cairn<ListedProject[]>("projects");
      const [diagnosis, ...reads] = await Promise.allSettled([
        cairn<LifecycleSummary>("diagnose"),
        ...listed.map((project) =>
          cairn<ProjectMemory[]>(memoryReadPath(project.project_id, project.role)),
        ),
      ]);
      const failed = [diagnosis, ...reads].find((result) => result.status === "rejected");
      if (failed?.status === "rejected" && failed.reason instanceof SessionRequiredError) {
        throw failed.reason;
      }
      const loaded: Decision[] = [];
      let partial = diagnosis.status === "rejected";
      reads.forEach((result, index) => {
        if (result.status === "rejected") {
          partial = true;
          return;
        }
        for (const memory of result.value as ProjectMemory[]) {
          loaded.push({
            ...memory,
            projectId: listed[index].project_id,
            projectName: listed[index].name,
          });
        }
      });
      return { listed, loaded, partial, diagnosis };
    }
    load()
      .then(({ listed, loaded, partial, diagnosis }) => {
        if (cancelled) {
          return;
        }
        setProjects(listed);
        setDecisions(loaded);
        setSummary(diagnosis.status === "fulfilled" ? diagnosis.value : null);
        setError(partial ? "Some figures could not be loaded and are left out." : null);
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
        const message = cause instanceof Error ? cause.message : "Could not load the overview";
        setLoadFailed(true);
        setError(`The overview could not be loaded. ${message}`);
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

  async function logout() {
    try {
      await cairn("auth/logout", { method: "POST" });
    } catch (cause) {
      if (cause instanceof SessionRequiredError) {
        setSessionRequired(true);
        router.replace("/");
        return;
      }
      const message = cause instanceof Error ? cause.message : "Logout failed";
      setError(`Could not log out. ${message}`);
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

  const perProject = projects.map((project) => ({
    label: project.name,
    href: `/projects/${project.project_id}`,
    value: decisions.filter((decision) => decision.projectId === project.project_id).length,
  }));
  const recent = newestFirst(decisions).slice(0, 8);

  return (
    <ConsoleShell
      title="Overview"
      action={
        <Button type="button" variant="outline" onClick={() => void logout()}>
          Log out
        </Button>
      }
    >
      <div className="flex min-w-0 flex-col gap-6">
        {error ? (
          <p role="alert" className="text-sm text-destructive">
            {error}
          </p>
        ) : null}
        {loading ? (
          <p role="status" className="text-sm text-muted-foreground">
            Loading overview…
          </p>
        ) : null}
        {!loading && !loadFailed ? (
          <>
            <section className="grid min-w-0 grid-cols-1 gap-4 md:grid-cols-2">
              <Card className="min-w-0">
                <CardHeader>
                  <CardTitle>
                    <h2>Memory by state</h2>
                  </CardTitle>
                  <CardDescription>Items stored for this account.</CardDescription>
                </CardHeader>
                <CardContent>
                  {summary ? (
                    <BarList
                      bars={[
                        { label: "Durable memories", value: summary.durable_memories },
                        { label: "Temporary observations", value: summary.temporary_observations },
                        { label: "Sensitive items", value: summary.sensitive_items },
                        { label: "Expired items", value: summary.expired_items },
                      ]}
                    />
                  ) : (
                    <p className="text-sm text-muted-foreground">Summary unavailable.</p>
                  )}
                </CardContent>
              </Card>
              <Card className="min-w-0">
                <CardHeader>
                  <CardTitle>
                    <h2>Decisions by project</h2>
                  </CardTitle>
                  <CardDescription>
                    {projects.length} {projects.length === 1 ? "project" : "projects"} ·{" "}
                    {decisions.length} {decisions.length === 1 ? "decision" : "decisions"}
                  </CardDescription>
                </CardHeader>
                <CardContent>
                  {projects.length === 0 ? (
                    <p className="text-sm text-muted-foreground">No projects for this account.</p>
                  ) : (
                    <BarList bars={perProject} />
                  )}
                </CardContent>
              </Card>
            </section>

            <section className="flex min-w-0 flex-col gap-3">
              <h2 className="text-base font-medium">Recent decisions</h2>
              {recent.length === 0 ? (
                <p className="text-sm text-muted-foreground">No decisions yet.</p>
              ) : (
                <ul className="flex flex-col gap-3">
                  {recent.map((decision) => (
                    <li key={`${decision.projectId}/${decision.memory_id}`} className="min-w-0">
                      <Card size="sm" className="min-w-0">
                        <CardContent className="flex min-w-0 flex-col gap-1">
                          <p className="line-clamp-2 text-sm [overflow-wrap:anywhere]">
                            {decision.content}
                          </p>
                          <p className="text-xs break-words text-muted-foreground">
                            <Link
                              href={`/projects/${decision.projectId}`}
                              className="rounded-sm underline-offset-4 outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
                            >
                              {decision.projectName}
                            </Link>{" "}
                            · {formatTime(decision.created_at)}
                          </p>
                        </CardContent>
                      </Card>
                    </li>
                  ))}
                </ul>
              )}
            </section>
          </>
        ) : null}
      </div>
    </ConsoleShell>
  );
}
