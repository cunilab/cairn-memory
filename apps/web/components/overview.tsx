"use client";

import { useEffect, useMemo, useState, type ReactNode } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { FolderPlusIcon, ShieldCheckIcon, SwordsIcon } from "lucide-react";
import { Bar, BarChart, CartesianGrid, Cell, LabelList, Pie, PieChart, XAxis, YAxis } from "recharts";
import { ConsoleShell } from "@/components/console-shell";
import { PageHeader } from "@/components/page-header";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  ChartContainer,
  ChartLegend,
  ChartLegendContent,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import {
  Empty,
  EmptyDescription,
  EmptyContent,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { cairn } from "@/lib/cairn";
import { memoryReadPath } from "@/lib/memory-path";
import { formatTime, newestFirst } from "@/lib/report";
import { SessionRequiredError } from "@/lib/session";
import type {
  LifecycleSummary,
  ListedProject,
  PersonalMemory,
  ProjectConflict,
  ProjectFreshnessEvent,
  ProjectMemory,
} from "@/lib/types";

type Decision = ProjectMemory & { projectId: string; projectName: string };

const DAY = 86400;
const ACTIVITY_DAYS = 14;

function kindLabel(kind: string): string {
  const text = kind.replace(/_/g, " ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

function Stat({
  label,
  value,
  hint,
  icon,
}: {
  label: string;
  value: ReactNode;
  hint?: ReactNode;
  icon?: ReactNode;
}) {
  return (
    <Card size="sm" className="min-w-0">
      <CardHeader>
        <CardDescription className="flex items-center justify-between gap-2">
          {label}
          {icon ? <span className="text-muted-foreground [&_svg]:size-4">{icon}</span> : null}
        </CardDescription>
        <CardTitle className="text-2xl font-semibold tabular-nums group-data-[size=sm]/card:text-2xl">
          {value}
        </CardTitle>
      </CardHeader>
      {hint ? (
        <CardContent className="text-xs text-muted-foreground">{hint}</CardContent>
      ) : null}
    </Card>
  );
}

/** A chart card. `summary` repeats the figures as text for screen readers. */
function ChartCard({
  title,
  description,
  summary,
  className,
  children,
}: {
  title: string;
  description?: string;
  summary: string;
  className?: string;
  children: ReactNode;
}) {
  return (
    <Card className={className}>
      <CardHeader>
        <CardTitle>
          <h2>{title}</h2>
        </CardTitle>
        {description ? <CardDescription>{description}</CardDescription> : null}
      </CardHeader>
      <CardContent>
        <figure className="m-0">
          {children}
          <figcaption className="sr-only">{summary}</figcaption>
        </figure>
      </CardContent>
    </Card>
  );
}

/** Value label at the end of a horizontal bar; LabelList skips zero-width bars. */
function EndLabel({ x, y, width, height, value }: { x?: number | string; y?: number | string; width?: number | string; height?: number | string; value?: unknown }) {
  return (
    <text
      x={Number(x) + Number(width) + 8}
      y={Number(y) + Number(height) / 2}
      dominantBaseline="central"
      className="fill-foreground text-xs tabular-nums"
    >
      {String(value ?? "")}
    </text>
  );
}

function LoadingOverview() {
  return (
    <div role="status" className="flex flex-col gap-4">
      <span className="sr-only">Loading overview…</span>
      <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
        {[0, 1, 2, 3].map((index) => (
          <Skeleton key={index} className="h-24" />
        ))}
      </div>
      <div className="grid gap-4 lg:grid-cols-3">
        <Skeleton className="h-72 lg:col-span-2" />
        <Skeleton className="h-72" />
      </div>
      <div className="grid gap-4 lg:grid-cols-2">
        <Skeleton className="h-64" />
        <Skeleton className="h-64" />
      </div>
    </div>
  );
}

export function Overview() {
  const router = useRouter();
  const [projects, setProjects] = useState<ListedProject[]>([]);
  const [summary, setSummary] = useState<LifecycleSummary | null>(null);
  const [decisions, setDecisions] = useState<Decision[]>([]);
  // Read once the data arrives: Date.now() cannot run while the page is prerendered.
  const [clock, setClock] = useState<{ now: number; today: number } | null>(null);
  const [captured, setCaptured] = useState<PersonalMemory[]>([]);
  const [conflicts, setConflicts] = useState<number | null>(null);
  const [staleEvents, setStaleEvents] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [sessionRequired, setSessionRequired] = useState(false);

  useResetOnHide(() => {
    setProjects([]);
    setSummary(null);
    setDecisions([]);
    setCaptured([]);
    setClock(null);
    setConflicts(null);
    setStaleEvents(null);
    setError(null);
    setLoading(true);
    setLoadFailed(false);
    setSessionRequired(false);
  });

  useEffect(() => {
    let cancelled = false;
    async function load() {
      const listed = await cairn<ListedProject[]>("projects");
      // Conflicts and freshness are owner reports; members do not get them.
      const owned = listed.filter((project) => project.role === "owner");
      // shortcut: export returns every memory of the account, fine while personal stores are small.
      const [diagnosis, exported, ...rest] = await Promise.allSettled([
        cairn<LifecycleSummary>("diagnose"),
        cairn<{ memories: PersonalMemory[]; forgottenIds?: string[] }>("export", { method: "POST" }),
        ...listed.map((project) =>
          cairn<ProjectMemory[]>(memoryReadPath(project.project_id, project.role)),
        ),
        ...owned.map((project) =>
          cairn<ProjectConflict[]>(`projects/${encodeURIComponent(project.project_id)}/conflicts`),
        ),
        ...owned.map((project) =>
          cairn<ProjectFreshnessEvent[]>(
            `projects/${encodeURIComponent(project.project_id)}/freshness`,
          ),
        ),
      ]);
      const reads = rest.slice(0, listed.length);
      const conflictReads = rest.slice(listed.length, listed.length + owned.length);
      const freshnessReads = rest.slice(listed.length + owned.length);
      const failed = [diagnosis, exported, ...rest].find((result) => result.status === "rejected");
      if (failed?.status === "rejected" && failed.reason instanceof SessionRequiredError) {
        throw failed.reason;
      }
      const loaded: Decision[] = [];
      let partial = diagnosis.status === "rejected" || exported.status === "rejected";
      let memories: PersonalMemory[] = [];
      if (exported.status === "fulfilled") {
        const forgotten = new Set(exported.value.forgottenIds ?? []);
        memories = exported.value.memories.filter((memory) => !forgotten.has(memory.memory_id));
      }
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
      let conflictCount: number | null = owned.length === 0 ? null : 0;
      for (const result of conflictReads) {
        if (result.status === "rejected") {
          partial = true;
          conflictCount = null;
          break;
        }
        conflictCount = (conflictCount ?? 0) + (result.value as ProjectConflict[]).length;
      }
      let freshnessCount: number | null = owned.length === 0 ? null : 0;
      for (const result of freshnessReads) {
        if (result.status === "rejected") {
          partial = true;
          freshnessCount = null;
          break;
        }
        freshnessCount = (freshnessCount ?? 0) + (result.value as ProjectFreshnessEvent[]).length;
      }
      return { listed, loaded, partial, diagnosis, memories, conflictCount, freshnessCount };
    }
    load()
      .then(({ listed, loaded, partial, diagnosis, memories, conflictCount, freshnessCount }) => {
        if (cancelled) {
          return;
        }
        setProjects(listed);
        setDecisions(loaded);
        setCaptured(memories);
        const midnight = new Date();
        midnight.setHours(0, 0, 0, 0);
        setClock({ now: Math.floor(Date.now() / 1000), today: Math.floor(midnight.getTime() / 1000) });
        setSummary(diagnosis.status === "fulfilled" ? diagnosis.value : null);
        setConflicts(conflictCount);
        setStaleEvents(freshnessCount);
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

  const figures = useMemo(() => {
    const { now, today } = clock ?? { now: 0, today: 0 };
    const activity = Array.from({ length: ACTIVITY_DAYS }, (_, index) => {
      const start = today - (ACTIVITY_DAYS - 1 - index) * DAY;
      return {
        start,
        day: new Date(start * 1000).toLocaleDateString(undefined, { month: "short", day: "numeric" }),
        captured: 0,
        decisions: 0,
      };
    });
    for (const memory of captured) {
      const created = Number(memory.created_at);
      const bucket = activity.find((item) => created >= item.start && created < item.start + DAY);
      if (bucket) {
        // Project-scoped rows are decisions; everything else was captured on a device.
        bucket[memory.scope === "project" ? "decisions" : "captured"] += 1;
      }
    }
    const kinds = new Map<string, number>();
    for (const memory of captured) {
      const kind = memory.kind ?? "explicit";
      kinds.set(kind, (kinds.get(kind) ?? 0) + 1);
    }
    const kindData = [...kinds]
      .sort((a, b) => b[1] - a[1])
      .map(([kind, count], index) => ({
        kind,
        label: kindLabel(kind),
        count,
        fill: `var(--chart-${(index % 5) + 1})`,
      }));
    const verified = decisions.filter((decision) => decision.verified_commit).length;
    const week = decisions.filter((decision) => Number(decision.created_at) >= now - 7 * DAY).length;
    const perProject = projects
      .map((project) => ({
        id: project.project_id,
        name: project.name,
        decisions: decisions.filter((decision) => decision.projectId === project.project_id).length,
      }))
      .sort((a, b) => b.decisions - a.decisions);
    return { activity, kindData, verified, week, perProject };
  }, [captured, clock, decisions, projects]);

  if (sessionRequired) {
    return (
      <main className="mx-auto flex min-h-svh w-full max-w-sm flex-1 items-center px-6">
        <p className="text-sm text-muted-foreground">Sign in is required.</p>
      </main>
    );
  }

  const recent = newestFirst(decisions).slice(0, 6);
  const stateData = summary
    ? [
        { state: "Durable memories", count: summary.durable_memories },
        { state: "Temporary observations", count: summary.temporary_observations },
        { state: "Sensitive items", count: summary.sensitive_items },
        { state: "Expired items", count: summary.expired_items },
      ]
    : [];
  const projectChartHeight = Math.max(160, figures.perProject.length * 40 + 24);

  const activityConfig = {
    captured: { label: "Captured", color: "var(--chart-2)" },
    decisions: { label: "Decisions", color: "var(--chart-1)" },
  } satisfies ChartConfig;
  const stateConfig = { count: { label: "Items", color: "var(--chart-2)" } } satisfies ChartConfig;
  const projectConfig = { decisions: { label: "Decisions", color: "var(--chart-1)" } } satisfies ChartConfig;
  const kindConfig = Object.fromEntries(
    figures.kindData.map((item) => [item.kind, { label: item.label, color: item.fill }]),
  ) satisfies ChartConfig;

  return (
    <ConsoleShell crumbs={[{ label: "Overview" }]}>
      <PageHeader
        title="Overview"
        description="What Cairn has kept for this account across your projects."
        actions={
          <Button variant="outline" nativeButton={false} render={<Link href="/projects" />}>
            All projects
          </Button>
        }
      />
      {error ? (
        <Alert variant="destructive">
          <AlertDescription className="text-destructive">{error}</AlertDescription>
        </Alert>
      ) : null}
      {loading ? <LoadingOverview /> : null}
      {!loading && !loadFailed && projects.length === 0 ? (
        <Empty className="border">
          <EmptyHeader>
            <EmptyMedia variant="icon">
              <FolderPlusIcon />
            </EmptyMedia>
            <EmptyTitle>No projects yet</EmptyTitle>
            <EmptyDescription>
              Projects appear here once an agent registers a repository with one of your devices.
              Create a device, then point the agent at it.
            </EmptyDescription>
          </EmptyHeader>
          <EmptyContent>
            <Button nativeButton={false} render={<Link href="/devices" />}>Set up a device</Button>
          </EmptyContent>
        </Empty>
      ) : null}
      {!loading && !loadFailed && projects.length > 0 ? (
        <>
          <section aria-label="Key figures" className="grid grid-cols-2 gap-3 lg:grid-cols-4 lg:gap-4">
            <Stat
              label="Projects"
              value={projects.length}
              hint={`${projects.filter((project) => project.role === "owner").length} owned`}
            />
            <Stat
              label="Decisions"
              value={decisions.length}
              hint={`${figures.week} added in the last 7 days`}
            />
            <Stat
              label="Verified"
              value={
                decisions.length === 0
                  ? "—"
                  : `${Math.round((figures.verified / decisions.length) * 100)}%`
              }
              icon={<ShieldCheckIcon />}
              hint={`${figures.verified} of ${decisions.length} tied to a commit`}
            />
            <Stat
              label="Conflicts"
              value={conflicts ?? "—"}
              icon={<SwordsIcon />}
              hint={
                conflicts === null
                  ? "Owner reports unavailable"
                  : `${staleEvents ?? 0} freshness ${staleEvents === 1 ? "event" : "events"}`
              }
            />
          </section>

          <section className="grid min-w-0 gap-4 lg:grid-cols-3">
            <ChartCard
              className="min-w-0 lg:col-span-2"
              title="Memory activity"
              description={`Memories saved per day, last ${ACTIVITY_DAYS} days.`}
              summary={`Memories per day: ${figures.activity.map((item) => `${item.day} ${item.captured} captured, ${item.decisions} decisions`).join("; ")}.`}
            >
              <ChartContainer config={activityConfig} className="aspect-auto h-60 w-full">
                <BarChart accessibilityLayer data={figures.activity} margin={{ left: -20, right: 4, top: 8 }}>
                  <CartesianGrid vertical={false} />
                  <XAxis
                    dataKey="day"
                    tickLine={false}
                    axisLine={false}
                    tickMargin={8}
                    interval="preserveStartEnd"
                    minTickGap={24}
                  />
                  <YAxis tickLine={false} axisLine={false} allowDecimals={false} width={44} />
                  <ChartTooltip cursor={false} content={<ChartTooltipContent />} />
                  <ChartLegend content={<ChartLegendContent />} />
                  <Bar dataKey="captured" stackId="a" fill="var(--color-captured)" maxBarSize={40} />
                  <Bar dataKey="decisions" stackId="a" fill="var(--color-decisions)" radius={[3, 3, 0, 0]} maxBarSize={40} />
                </BarChart>
              </ChartContainer>
            </ChartCard>

            <ChartCard
              className="min-w-0"
              title="Memory kinds"
              description="Everything stored for this account, by type."
              summary={`Memories by kind: ${figures.kindData.map((item) => `${item.label} ${item.count}`).join(", ")}.`}
            >
              {figures.kindData.length === 0 ? (
                <p className="py-10 text-center text-sm text-muted-foreground">Nothing saved yet.</p>
              ) : (
                <div className="flex flex-col items-center gap-4">
                  <ChartContainer config={kindConfig} className="aspect-square h-40">
                    <PieChart>
                      <ChartTooltip content={<ChartTooltipContent hideLabel nameKey="kind" />} />
                      <Pie
                        data={figures.kindData}
                        dataKey="count"
                        nameKey="kind"
                        innerRadius={48}
                        outerRadius={72}
                        strokeWidth={2}
                        stroke="var(--card)"
                      >
                        {figures.kindData.map((item) => (
                          <Cell key={item.kind} fill={item.fill} />
                        ))}
                      </Pie>
                    </PieChart>
                  </ChartContainer>
                  <ul className="flex w-full flex-col gap-1.5 text-sm">
                    {figures.kindData.map((item) => (
                      <li key={item.kind} className="flex items-center gap-2">
                        <span
                          aria-hidden
                          className="size-2.5 shrink-0 rounded-sm"
                          style={{ background: item.fill }}
                        />
                        <span className="min-w-0 flex-1 truncate">{item.label}</span>
                        <span className="tabular-nums text-muted-foreground">{item.count}</span>
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </ChartCard>
          </section>

          <section className="grid min-w-0 gap-4 lg:grid-cols-2">
            <ChartCard
              className="min-w-0"
              title="Memory by state"
              description="Items stored for this account."
              summary={`Memory by state: ${stateData.map((item) => `${item.state} ${item.count}`).join(", ")}.`}
            >
              {summary ? (
                <ChartContainer config={stateConfig} className="aspect-auto h-52 w-full">
                  <BarChart
                    accessibilityLayer
                    data={stateData}
                    layout="vertical"
                    margin={{ left: 0, right: 24 }}
                  >
                    <CartesianGrid horizontal={false} />
                    <YAxis
                      dataKey="state"
                      type="category"
                      tickLine={false}
                      axisLine={false}
                      width={150}
                      tickFormatter={(value: string) => value}
                    />
                    <XAxis type="number" hide />
                    <ChartTooltip cursor={false} content={<ChartTooltipContent hideLabel />} />
                    <Bar dataKey="count" fill="var(--color-count)" radius={3}>
                      <LabelList dataKey="count" content={EndLabel} />
                    </Bar>
                  </BarChart>
                </ChartContainer>
              ) : (
                <p className="py-10 text-center text-sm text-muted-foreground">Summary unavailable.</p>
              )}
            </ChartCard>

            <ChartCard
              className="min-w-0"
              title="Decisions by project"
              description={`${projects.length} ${projects.length === 1 ? "project" : "projects"}, ${decisions.length} ${decisions.length === 1 ? "decision" : "decisions"}.`}
              summary={`Decisions by project: ${figures.perProject.map((item) => `${item.name} ${item.decisions}`).join(", ")}.`}
            >
              <ChartContainer
                config={projectConfig}
                className="aspect-auto w-full"
                style={{ height: projectChartHeight }}
              >
                <BarChart
                  accessibilityLayer
                  data={figures.perProject}
                  layout="vertical"
                  margin={{ left: 0, right: 24 }}
                >
                  <CartesianGrid horizontal={false} />
                  <YAxis
                    dataKey="name"
                    type="category"
                    tickLine={false}
                    axisLine={false}
                    width={110}
                    tickFormatter={(value: string) => (value.length > 16 ? `${value.slice(0, 15)}…` : value)}
                  />
                  <XAxis type="number" hide />
                  <ChartTooltip cursor={false} content={<ChartTooltipContent hideLabel />} />
                  <Bar
                    dataKey="decisions"
                    fill="var(--color-decisions)"
                    radius={3}
                    cursor="pointer"
                    onClick={(item) => {
                      const id = (item as { payload?: { id?: string } }).payload?.id;
                      if (id) {
                        router.push(`/projects/${id}`);
                      }
                    }}
                  >
                    <LabelList dataKey="decisions" content={EndLabel} />
                  </Bar>
                </BarChart>
              </ChartContainer>
            </ChartCard>
          </section>

          <section className="flex min-w-0 flex-col gap-3" aria-labelledby="recent-decisions">
            <h2 id="recent-decisions" className="text-base font-medium">
              Recent decisions
            </h2>
            {recent.length === 0 ? (
              <Empty className="border">
                <EmptyHeader>
                  <EmptyTitle>No decisions yet</EmptyTitle>
                  <EmptyDescription>
                    Promote a memory in a project to record its first decision.
                  </EmptyDescription>
                </EmptyHeader>
              </Empty>
            ) : (
              <ul className="divide-y overflow-hidden rounded-lg border bg-card">
                {recent.map((decision) => (
                  <li
                    key={`${decision.projectId}/${decision.memory_id}`}
                    className="flex min-w-0 flex-col gap-1.5 px-4 py-3 sm:flex-row sm:items-start sm:gap-4"
                  >
                    <div className="flex min-w-0 flex-1 flex-col gap-1">
                      <p className="line-clamp-2 text-sm [overflow-wrap:anywhere]">{decision.content}</p>
                      <p className="text-xs text-muted-foreground">
                        <Link
                          href={`/projects/${decision.projectId}`}
                          className="rounded-sm font-medium text-foreground underline-offset-4 outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
                        >
                          {decision.projectName}
                        </Link>
                        <span aria-hidden> / </span>
                        <span className="sr-only">, </span>
                        {formatTime(decision.created_at)}
                      </p>
                    </div>
                    {decision.verified_commit ? <Badge variant="success">Verified</Badge> : null}
                  </li>
                ))}
              </ul>
            )}
          </section>
        </>
      ) : null}
    </ConsoleShell>
  );
}
