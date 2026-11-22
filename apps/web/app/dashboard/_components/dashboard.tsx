"use client";

import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { TriangleAlertIcon } from "lucide-react";
import { Area, AreaChart, CartesianGrid, XAxis, YAxis } from "recharts";
import { ErrorAlert, LoadingBlock } from "@/components/states";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from "@/components/ui/chart";
import { useOpenMemory } from "@/hooks/use-query-params";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { FilterSelect } from "@/components/filter-select";
import { cairn } from "@/lib/cairn";
import type { Stats } from "@/lib/generated/Stats";
import { LOGIN_PATH, SessionRequiredError } from "@/lib/session";
import { healthLabel, healthSegments, valueSentence } from "../_lib/summary";
import { BarList } from "./bar-list";
import { FirstRun } from "./first-run";

const POLL_MS = 5000;
const POLL_LIMIT_MS = 15 * 60 * 1000;
const SEGMENT_COLOR: Record<string, string> = {
  active: "bg-success",
  stale: "bg-primary",
  superseded: "bg-muted-foreground/50",
};
const RANGES = [7, 30, 90] as const;
const RANGE_OPTIONS = RANGES.map((n) => ({ value: String(n), label: `Last ${n} days` }));
const tokenConfig = { tokens: { label: "Tokens delivered (estimated)", color: "var(--chart-1)" } } satisfies ChartConfig;

function kindLabel(kind: string): string {
  const text = kind.replace(/_/g, " ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

function Section({ title, description, children, className }: { title: string; description?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <Card className={className}>
      <CardHeader>
        <CardTitle>
          <h2>{title}</h2>
        </CardTitle>
        {description ? <CardDescription>{description}</CardDescription> : null}
      </CardHeader>
      <CardContent>{children}</CardContent>
    </Card>
  );
}

function UsageChart({ stats, days }: { stats: Stats; days: number }) {
  const usage = stats.usage_days;
  const data = usage.days.map((d) => ({ ...d, label: d.day.slice(5) }));
  const summary = valueSentence(usage, days);
  return (
    <Section title="Tokens delivered per day" description={`Estimated: bytes ÷ 4. Your usage, last ${days} days.`}>
      {usage.lookups > 0 ? (
        <figure className="m-0">
          <div role="img" aria-label={summary}>
            <ChartContainer config={tokenConfig} className="aspect-auto h-52 w-full sm:h-60">
              <AreaChart data={data} margin={{ left: -12, right: 4, top: 8 }}>
                <CartesianGrid vertical={false} />
                <XAxis dataKey="label" tickLine={false} axisLine={false} tickMargin={8} minTickGap={32} />
                <YAxis tickLine={false} axisLine={false} width={44} allowDecimals={false} />
                <ChartTooltip cursor={false} content={<ChartTooltipContent />} />
                <Area dataKey="tokens" type="monotone" stroke="var(--color-tokens)" fill="var(--color-tokens)" fillOpacity={0.2} isAnimationActive={false} />
              </AreaChart>
            </ChartContainer>
          </div>
          <table className="sr-only">
            <caption>Tokens delivered per day (estimated)</caption>
            <thead>
              <tr><th scope="col">Day</th><th scope="col">Tokens</th><th scope="col">Lookups</th></tr>
            </thead>
            <tbody>
              {data.map((d) => (
                <tr key={d.day}><th scope="row">{d.day}</th><td>{d.tokens}</td><td>{d.lookups}</td></tr>
              ))}
            </tbody>
          </table>
        </figure>
      ) : (
        <div className="flex h-52 items-center justify-center rounded-md border border-dashed text-sm text-muted-foreground sm:h-60">
          Nothing delivered yet.
        </div>
      )}
    </Section>
  );
}

export function Dashboard() {
  const router = useRouter();
  const openMemory = useOpenMemory();
  const [stats, setStats] = useState<Stats | null>(null);
  const [days, setDays] = useState<number>(30);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [gaveUp, setGaveUp] = useState(false);

  useResetOnHide(() => {
    setStats(null);
    setError(null);
    setGaveUp(false);
  });

  const load = useCallback(
    () =>
      cairn<Stats>(`stats?tz=${-new Date().getTimezoneOffset()}&days=${days}`).then(
        (next) => {
          setStats(next);
          setError(null);
        },
        (cause: unknown) => {
          if (cause instanceof SessionRequiredError) {
            router.replace(LOGIN_PATH);
            return;
          }
          setError(`Stats could not be loaded. ${cause instanceof Error ? cause.message : "Check that the server is running, then retry."}`);
        },
      ),
    [router, days],
  );

  useEffect(() => {
    // The load resolves asynchronously; state is set from its callbacks, not here.
    void load();
  }, [load, attempt]);

  // First run: poll while the tab is visible, give up after 15 minutes.
  const firstRun = stats?.first_run === true;
  useEffect(() => {
    if (!firstRun) return;
    const started = Date.now();
    const timer = setInterval(() => {
      if (document.visibilityState !== "visible") return;
      if (Date.now() - started > POLL_LIMIT_MS) {
        setGaveUp(true);
        clearInterval(timer);
        return;
      }
      void load();
    }, POLL_MS);
    return () => clearInterval(timer);
  }, [firstRun, load]);

  const segments = useMemo(() => healthSegments(stats?.memories.by_status ?? {}), [stats]);
  const kinds = useMemo(
    () =>
      Object.entries(stats?.memories.by_kind ?? {})
        .sort((a, b) => b[1] - a[1])
        .map(([kind, value]) => ({ label: kindLabel(kind), value, href: `/memories?kind=${encodeURIComponent(kind)}` })),
    [stats],
  );

  if (error && !stats) {
    return <ErrorAlert message={error} onRetry={() => { setError(null); setAttempt((n) => n + 1); }} />;
  }
  if (stats === null) return <LoadingBlock label="Loading dashboard…" rows={5} />;

  const attention: { key: string; text: string; href: string; action: string }[] = [
    ...stats.devices_attention.map((d) => ({ key: d.device_id, text: `${d.name}: ${d.reason}`, href: "/settings?tab=devices", action: "Devices" })),
    ...(stats.jobs.quarantined > 0
      ? [{
          key: "jobs",
          text: `${stats.jobs.quarantined} ${stats.jobs.quarantined === 1 ? "job" : "jobs"} stopped retrying.`,
          href: "/activity?tab=jobs",
          action: "Activity",
        }]
      : []),
  ];

  return (
    <div className="flex min-w-0 flex-col gap-4">
      {firstRun ? (
        <FirstRun
          checkedIn={stats.devices_checked_in > 0}
          captured={stats.capture.last_at !== null}
          total={stats.memories.total}
          gaveUp={gaveUp}
        />
      ) : (
        <>
          <div className="flex flex-wrap items-end justify-between gap-3">
            <p className="max-w-prose text-base">{valueSentence(stats.usage_days, days)}</p>
            <div className="w-40">
              <FilterSelect label="Range" value={String(days)} onChange={(v) => setDays(Number(v) || 30)} options={RANGE_OPTIONS} allLabel="Last 30 days" />
            </div>
          </div>
          <UsageChart stats={stats} days={days} />
        </>
      )}
      {error ? <ErrorAlert message={error} onRetry={() => void load()} /> : null}
      {attention.length > 0 ? (
        <section aria-label="Needs attention" className="flex flex-col gap-2">
          {attention.map((item) => (
            <Alert key={item.key}>
              <TriangleAlertIcon aria-hidden />
              <AlertDescription className="flex flex-wrap items-center justify-between gap-2">
                <span className="min-w-0 [overflow-wrap:anywhere]">{item.text}</span>
                <Button nativeButton={false} variant="outline" size="sm" render={<Link href={item.href} />}>
                  {item.action}
                </Button>
              </AlertDescription>
            </Alert>
          ))}
        </section>
      ) : null}
      <div className="grid min-w-0 gap-4 lg:grid-cols-2">
        <Section title="Memory health">
          <div className="flex flex-col gap-3">
            <p className="text-2xl font-semibold tabular-nums">
              {stats.memories.total.toLocaleString()} {stats.memories.total === 1 ? "memory" : "memories"}
            </p>
            <div role="img" aria-label={healthLabel(segments)} className="flex h-3 w-full overflow-hidden rounded-full bg-muted">
              {segments.map((s) => (
                <span key={s.key} className={SEGMENT_COLOR[s.key]} style={{ width: `${(s.count / stats.memories.total) * 100}%` }} />
              ))}
            </div>
            <ul className="flex flex-wrap gap-x-4 gap-y-1 text-sm">
              {segments.map((s) => (
                <li key={s.key}>
                  <Link href={`/memories?status=${s.key}`} className="rounded-sm underline-offset-4 outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50">
                    {s.label} <span className="tabular-nums">{s.count.toLocaleString()}</span>
                  </Link>
                </li>
              ))}
            </ul>
            <p className="text-sm text-muted-foreground tabular-nums">
              {Math.round(stats.memory_health.linked_ratio * 100)}% of durable memories linked · stale: {stats.memory_health.stale_missing} file gone, {stats.memory_health.stale_feedback} reported wrong · {stats.memory_health.expiring} expiring
            </p>
            <p className="text-sm text-muted-foreground">
              <Link href="/projects" className="underline-offset-4 hover:underline">
                {stats.projects} {stats.projects === 1 ? "project" : "projects"}
              </Link>
            </p>
          </div>
        </Section>
        <Section title="Usage" description="Your usage in the last 30 days, whatever range is selected. Counts memories, not requests.">
          <dl className="grid grid-cols-[1fr_auto] gap-x-4 gap-y-1 text-sm tabular-nums">
            <dt>Different memories fetched</dt><dd>{stats.usage.fetched_30d.toLocaleString()}</dd>
            <dt>Marked helpful</dt><dd>{stats.usage.helpful_30d.toLocaleString()}</dd>
            <dt>Durable memories never fetched</dt>
            <dd>{stats.usage.never_fetched.toLocaleString()}</dd>
          </dl>
          {stats.usage.top.length > 0 ? (
            <ul className="mt-3 flex flex-col gap-1 text-sm">
              {stats.usage.top.map((m) => (
                <li key={m.id} className="flex items-baseline justify-between gap-3">
                  <button type="button" className="min-w-0 rounded-sm text-left break-words underline-offset-4 outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50" onClick={() => openMemory(m.id)}>
                    {kindLabel(m.kind)} · {m.title}
                  </button>
                  <span className="tabular-nums text-muted-foreground">{m.fetched}</span>
                </li>
              ))}
            </ul>
          ) : null}
        </Section>
        <Section title="Benchmark" description="Current version, 2,000 distractor memories, 43 questions. These are benchmark figures, not your usage.">
          <dl className="grid grid-cols-2 gap-3 text-sm tabular-nums">
            <div><dt className="text-muted-foreground">Right memory in top 5</dt><dd className="text-lg font-semibold">{Math.round(stats.bench.hit_at_5 * 100)}%</dd></div>
            <div><dt className="text-muted-foreground">Search reply</dt><dd className="text-lg font-semibold">~{stats.bench.search_tokens.toLocaleString()} tokens</dd></div>
            <div><dt className="text-muted-foreground">Session overhead</dt><dd className="text-lg font-semibold">~{stats.bench.session_tokens.toLocaleString()} tokens</dd></div>
            <div><dt className="text-muted-foreground">Fetch one</dt><dd className="text-lg font-semibold">~{stats.bench.get_tokens.toLocaleString()} tokens</dd></div>
          </dl>
        </Section>
        <Section title="By kind" description="Memories by kind. Select one to list them.">
          {kinds.length === 0 ? (
            <p className="text-sm text-muted-foreground">Nothing saved yet.</p>
          ) : (
            <BarList bars={kinds} />
          )}
        </Section>
      </div>
    </div>
  );
}
