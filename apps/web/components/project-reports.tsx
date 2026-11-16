"use client";

import { useEffect, useState } from "react";
import { BarList } from "@/components/bar-list";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { IdText } from "@/components/badges";
import type { ReactNode } from "react";
import { cairn } from "@/lib/cairn";
import { formatTime } from "@/lib/report";
import { SessionRequiredError } from "@/lib/session";
import type {
  ProjectConflict,
  ProjectFreshnessEvent,
  ProjectMemory,
  ProjectRevocation,
  ShareCandidate,
} from "@/lib/types";

type Section<T> = { data: T } | { error: string };

type Reports = {
  conflicts: Section<ProjectConflict[]>;
  freshness: Section<ProjectFreshnessEvent[]>;
  revocations: Section<ProjectRevocation[]>;
  candidates: Section<Array<{ email: string; items: ShareCandidate[] }>>;
};

async function settle<T>(request: Promise<T>): Promise<Section<T>> {
  try {
    return { data: await request };
  } catch (cause) {
    if (cause instanceof SessionRequiredError) {
      throw cause;
    }
    return { error: cause instanceof Error ? cause.message : "Request failed" };
  }
}

function Empty({ children }: { children: string }) {
  return <p className="text-sm text-muted-foreground">{children}</p>;
}

function Failed({ error }: { error: string }) {
  return (
    <Alert variant="destructive">
      <AlertDescription className="text-destructive">{error}</AlertDescription>
    </Alert>
  );
}

function ReportCard({
  id,
  title,
  description,
  count,
  className,
  children,
}: {
  id: string;
  title: string;
  description: string;
  count?: number;
  className?: string;
  children: ReactNode;
}) {
  return (
    <section aria-labelledby={id} className={className}>
      <Card className="h-full min-w-0">
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <h3 id={id}>{title}</h3>
            {count ? <Badge variant="secondary">{count}</Badge> : null}
          </CardTitle>
          <CardDescription>{description}</CardDescription>
        </CardHeader>
        <CardContent className="flex min-w-0 flex-col gap-3">{children}</CardContent>
      </Card>
    </section>
  );
}

/** Read-only owner reports. `decisions` only triggers a reload after an owner action. */
export function ProjectReports({
  projectId,
  memberEmails,
  decisions,
  onSessionRequired,
}: {
  projectId: string;
  memberEmails: string[];
  decisions: ProjectMemory[];
  onSessionRequired: () => void;
}) {
  const [reports, setReports] = useState<Reports | null>(null);
  const emailsKey = memberEmails.join("\n");

  useEffect(() => {
    let cancelled = false;
    const base = `projects/${encodeURIComponent(projectId)}`;
    const emails = emailsKey ? emailsKey.split("\n") : [];
    Promise.all([
      settle(cairn<ProjectConflict[]>(`${base}/conflicts`)),
      settle(cairn<ProjectFreshnessEvent[]>(`${base}/freshness`)),
      settle(cairn<ProjectRevocation[]>(`${base}/revocations`)),
      settle(
        Promise.all(
          emails.map(async (email) => ({
            email,
            items: await cairn<ShareCandidate[]>(
              `${base}/share-candidates?email=${encodeURIComponent(email)}`,
            ),
          })),
        ),
      ),
    ])
      .then(([conflicts, freshness, revocations, candidates]) => {
        if (!cancelled) {
          setReports({ conflicts, freshness, revocations, candidates });
        }
      })
      .catch((cause: unknown) => {
        if (!cancelled && cause instanceof SessionRequiredError) {
          onSessionRequired();
        }
      });
    return () => {
      cancelled = true;
    };
  }, [projectId, emailsKey, decisions, onSessionRequired]);

  if (!reports) {
    return (
      <div role="status" className="grid gap-4 lg:grid-cols-2">
        <span className="sr-only">Loading reports…</span>
        <Skeleton className="h-40" />
        <Skeleton className="h-40" />
        <Skeleton className="h-40" />
        <Skeleton className="h-40" />
      </div>
    );
  }

  const actions = new Map<string, number>();
  if ("data" in reports.freshness) {
    for (const event of reports.freshness.data) {
      actions.set(event.action, (actions.get(event.action) ?? 0) + 1);
    }
  }

  return (
    <div className="grid min-w-0 gap-4 lg:grid-cols-2">
      <ReportCard
        id="report-conflicts"
        title="Conflicts"
        description="Decisions that compete with each other."
        count={"data" in reports.conflicts ? reports.conflicts.data.length : undefined}
      >
        {"error" in reports.conflicts ? (
          <Failed error={reports.conflicts.error} />
        ) : reports.conflicts.data.length === 0 ? (
          <Empty>No competing decisions.</Empty>
        ) : (
          <ul className="divide-y rounded-lg border">
            {reports.conflicts.data.map((conflict) => (
              <li key={conflict.memory_ids.join("|")} className="px-3 py-2">
                <IdText id={conflict.memory_ids.join(" · ")} />
              </li>
            ))}
          </ul>
        )}
      </ReportCard>

      <ReportCard
        id="report-freshness"
        title="Freshness"
        description="How decisions were re-checked against the code."
      >
        {"error" in reports.freshness ? (
          <Failed error={reports.freshness.error} />
        ) : reports.freshness.data.length === 0 ? (
          <Empty>No freshness events.</Empty>
        ) : (
          <>
            <BarList bars={[...actions].map(([label, value]) => ({ label, value }))} />
            <ul className="divide-y rounded-lg border">
              {[...reports.freshness.data]
                .reverse()
                .slice(0, 10)
                .map((event) => (
                  <li key={event.event_id} className="flex min-w-0 flex-col gap-0.5 px-3 py-2 text-sm">
                    <p className="[overflow-wrap:anywhere]">
                      <IdText id={event.memory_id} className="text-foreground" /> · {event.action}
                    </p>
                    <p className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
                      {event.previous_commit ? `${event.previous_commit} → ` : ""}
                      {event.commit} · {formatTime(event.created_at)}
                      {event.reason ? ` · ${event.reason}` : ""}
                    </p>
                  </li>
                ))}
            </ul>
          </>
        )}
      </ReportCard>

      <ReportCard
        id="report-candidates"
        title="Share candidates"
        description="Decisions that could be shared with each team member."
      >
        {"error" in reports.candidates ? (
          <Failed error={reports.candidates.error} />
        ) : reports.candidates.data.length === 0 ? (
          <Empty>Add a team member to see decisions that could be shared.</Empty>
        ) : (
          reports.candidates.data.map(({ email, items }) => (
            <div key={email} className="flex min-w-0 flex-col gap-2">
              <h4 className="text-sm font-medium break-all">{email}</h4>
              {items.length === 0 ? (
                <Empty>No candidates for this member.</Empty>
              ) : (
                <ul className="divide-y rounded-lg border">
                  {items.map((item) => (
                    <li key={item.memory_id} className="flex min-w-0 flex-col gap-0.5 px-3 py-2 text-sm">
                      <IdText id={item.memory_id} className="text-foreground" />
                      <p className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
                        {item.reason}. {item.not_auto_shared_reason}
                      </p>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          ))
        )}
      </ReportCard>

      <ReportCard
        id="report-revocations"
        title="Revoked shares"
        description="Shares you took back from a member."
        count={"data" in reports.revocations ? reports.revocations.data.length : undefined}
      >
        {"error" in reports.revocations ? (
          <Failed error={reports.revocations.error} />
        ) : reports.revocations.data.length === 0 ? (
          <Empty>No revoked shares.</Empty>
        ) : (
          <ul className="divide-y rounded-lg border">
            {reports.revocations.data.map((share) => (
              <li
                key={`${share.memory_id}/${share.member_person_id}/${share.created_at}`}
                className="flex min-w-0 flex-col gap-0.5 px-3 py-2 text-sm"
              >
                <IdText id={share.memory_id} className="text-foreground" />
                <p className="text-xs break-all text-muted-foreground">
                  {share.member_email ?? share.member_person_id} · revoked {formatTime(share.revoked_at)}
                </p>
              </li>
            ))}
          </ul>
        )}
      </ReportCard>
    </div>
  );
}
