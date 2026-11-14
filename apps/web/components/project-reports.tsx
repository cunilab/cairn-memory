"use client";

import { useEffect, useState } from "react";
import { BarList } from "@/components/bar-list";
import { Card, CardContent } from "@/components/ui/card";
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
    <p role="alert" className="text-sm text-destructive">
      {error}
    </p>
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
      <p role="status" className="text-sm text-muted-foreground">
        Loading reports…
      </p>
    );
  }

  const actions = new Map<string, number>();
  if ("data" in reports.freshness) {
    for (const event of reports.freshness.data) {
      actions.set(event.action, (actions.get(event.action) ?? 0) + 1);
    }
  }

  return (
    <>
      <section className="flex min-w-0 flex-col gap-3" aria-labelledby="report-conflicts">
        <h2 id="report-conflicts" className="text-base font-medium">
          Conflicts
        </h2>
        {"error" in reports.conflicts ? (
          <Failed error={reports.conflicts.error} />
        ) : reports.conflicts.data.length === 0 ? (
          <Empty>No competing decisions.</Empty>
        ) : (
          <ul className="flex flex-col gap-3">
            {reports.conflicts.data.map((conflict) => (
              <li key={conflict.memory_ids.join("|")} className="min-w-0">
                <Card size="sm" className="min-w-0">
                  <CardContent className="text-sm break-all">
                    {conflict.memory_ids.join(" · ")}
                  </CardContent>
                </Card>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="flex min-w-0 flex-col gap-3" aria-labelledby="report-freshness">
        <h2 id="report-freshness" className="text-base font-medium">
          Freshness
        </h2>
        {"error" in reports.freshness ? (
          <Failed error={reports.freshness.error} />
        ) : reports.freshness.data.length === 0 ? (
          <Empty>No freshness events.</Empty>
        ) : (
          <>
            <BarList bars={[...actions].map(([label, value]) => ({ label, value }))} />
            <ul className="flex flex-col gap-3">
              {[...reports.freshness.data]
                .reverse()
                .slice(0, 10)
                .map((event) => (
                  <li key={event.event_id} className="min-w-0">
                    <Card size="sm" className="min-w-0">
                      <CardContent className="flex min-w-0 flex-col gap-1 text-sm">
                        <p className="[overflow-wrap:anywhere]">
                          {event.memory_id} · {event.action}
                        </p>
                        <p className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
                          {event.previous_commit ? `${event.previous_commit} → ` : ""}
                          {event.commit} · {formatTime(event.created_at)}
                          {event.reason ? ` · ${event.reason}` : ""}
                        </p>
                      </CardContent>
                    </Card>
                  </li>
                ))}
            </ul>
          </>
        )}
      </section>

      <section className="flex min-w-0 flex-col gap-3" aria-labelledby="report-candidates">
        <h2 id="report-candidates" className="text-base font-medium">
          Share candidates
        </h2>
        {"error" in reports.candidates ? (
          <Failed error={reports.candidates.error} />
        ) : reports.candidates.data.length === 0 ? (
          <Empty>Add a team member to see decisions that could be shared.</Empty>
        ) : (
          reports.candidates.data.map(({ email, items }) => (
            <div key={email} className="flex min-w-0 flex-col gap-2">
              <h3 className="text-sm font-medium break-all">{email}</h3>
              {items.length === 0 ? (
                <Empty>No candidates for this member.</Empty>
              ) : (
                <ul className="flex flex-col gap-3">
                  {items.map((item) => (
                    <li key={item.memory_id} className="min-w-0">
                      <Card size="sm" className="min-w-0">
                        <CardContent className="flex min-w-0 flex-col gap-1 text-sm">
                          <p className="break-all">{item.memory_id}</p>
                          <p className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
                            {item.reason}. {item.not_auto_shared_reason}
                          </p>
                        </CardContent>
                      </Card>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          ))
        )}
      </section>

      <section className="flex min-w-0 flex-col gap-3" aria-labelledby="report-revocations">
        <h2 id="report-revocations" className="text-base font-medium">
          Revoked shares
        </h2>
        {"error" in reports.revocations ? (
          <Failed error={reports.revocations.error} />
        ) : reports.revocations.data.length === 0 ? (
          <Empty>No revoked shares.</Empty>
        ) : (
          <ul className="flex flex-col gap-3">
            {reports.revocations.data.map((share) => (
              <li
                key={`${share.memory_id}/${share.member_person_id}/${share.created_at}`}
                className="min-w-0"
              >
                <Card size="sm" className="min-w-0">
                  <CardContent className="flex min-w-0 flex-col gap-1 text-sm">
                    <p className="break-all">{share.memory_id}</p>
                    <p className="text-xs break-all text-muted-foreground">
                      {share.member_person_id} · revoked {formatTime(share.revoked_at)}
                    </p>
                  </CardContent>
                </Card>
              </li>
            ))}
          </ul>
        )}
      </section>
    </>
  );
}
