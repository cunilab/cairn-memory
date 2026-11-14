"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { ConsoleShell } from "@/components/console-shell";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { cairn } from "@/lib/cairn";
import { SessionRequiredError } from "@/lib/session";
import type { ListedProject } from "@/lib/types";

function roleLabel(role: string): string {
  if (role === "owner") {
    return "Owner";
  }
  if (role === "member") {
    return "Member";
  }
  return role;
}

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

  return (
    <ConsoleShell
      title="Projects"
      action={
        <Button type="button" variant="outline" onClick={() => void logout()}>
          Log out
        </Button>
      }
    >
      <div className="flex flex-col gap-4">
        {error ? (
          <p role="alert" className="text-sm text-destructive">
            {error}
          </p>
        ) : null}
        {loading ? (
          <p role="status" className="text-sm text-muted-foreground">
            Loading projects…
          </p>
        ) : null}
        {!loading && projects.length === 0 && !error ? (
          <div className="flex w-full min-w-0 flex-col items-center justify-center gap-2 rounded-xl border border-dashed p-6 text-center">
            <p className="text-sm font-medium">No projects</p>
            <p className="max-w-sm text-sm text-balance text-muted-foreground">
              No projects for this account.
            </p>
          </div>
        ) : null}
        <ul className="flex flex-col gap-3">
          {projects.map((project) => (
            <li key={project.project_id}>
              <Link
                href={`/projects/${project.project_id}`}
                className="block rounded-xl outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
              >
                <Card className="h-full hover:bg-muted/40">
                  <CardHeader>
                    <CardTitle className="break-words">{project.name}</CardTitle>
                    <CardDescription className="break-words">
                      {project.repository} · {roleLabel(project.role)}
                    </CardDescription>
                  </CardHeader>
                  <CardContent className="break-all text-sm text-muted-foreground">
                    {project.project_id}
                  </CardContent>
                </Card>
              </Link>
            </li>
          ))}
        </ul>
      </div>
    </ConsoleShell>
  );
}
