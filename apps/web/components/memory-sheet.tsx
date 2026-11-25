"use client";

import { Pin } from "lucide-react";
import { Fragment, useCallback, useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { toast } from "sonner";
import { KindBadge, StatusBadge } from "@/components/badges";
import { CopyButton } from "@/components/copy-button";
import { ErrorAlert, LoadingBlock } from "@/components/states";
import { Time } from "@/components/time";
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
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Separator } from "@/components/ui/separator";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { useQueryParams } from "@/hooks/use-query-params";
import { useAccount } from "@/hooks/use-account";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { forgetMemoryImpact } from "@/lib/action-copy";
import { ApiError, cairn } from "@/lib/cairn";
import { announceMemoriesChanged } from "@/lib/memory-events";
import { LOGIN_PATH, SessionRequiredError } from "@/lib/session";
import type { LinkView } from "@/lib/generated/LinkView";
import type { MemoryView } from "@/lib/generated/MemoryView";
import { memoryLabel } from "@/app/memories/_components/memory-label";

const OUT_LABEL: Record<string, string> = {
  relates: "Related",
  causes: "Causes",
  fixes: "Fixes",
  depends_on: "Depends on",
  supersedes: "Supersedes",
  contradicts: "Contradicts",
};
const IN_LABEL: Record<string, string> = {
  relates: "Related",
  causes: "Caused by",
  fixes: "Fixed by",
  depends_on: "Needed by",
  supersedes: "Superseded by",
  contradicts: "Contradicts",
};

function words(key: string): string {
  const text = key.replaceAll("_", " ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="flex flex-col gap-2">
      <Separator />
      <h3 className="pt-2 text-sm font-semibold">{title}</h3>
      {children}
    </section>
  );
}

function Lineage({
  memory,
  open,
}: {
  memory: MemoryView;
  open: (id: string) => void;
}) {
  const groups = new Map<string, LinkView[]>();
  for (const link of memory.links) {
    const incoming = link.direction === "in";
    const name = (incoming ? IN_LABEL : OUT_LABEL)[link.relation] ?? words(link.relation);
    groups.set(name, [...(groups.get(name) ?? []), link]);
  }
  if (groups.size === 0) {
    return null;
  }
  return (
    <Section title="Lineage">
      <dl className="flex flex-col gap-2 text-sm">
        {[...groups].map(([name, links]) => (
          <div key={name}>
            <dt className="text-xs font-medium text-muted-foreground">{name}</dt>
            {links.map((link) => {
              const otherId = link.other?.id ?? (link.direction === "out" ? link.target : link.source);
              return (
                <dd key={`${link.source}:${link.relation}:${link.target}`} className="flex items-center gap-2 py-0.5">
                  {link.other ? <KindBadge kind={link.other.kind} /> : null}
                  <button
                    type="button"
                    className="min-w-0 rounded-sm text-left break-words underline-offset-4 outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
                    onClick={() => open(otherId)}
                  >
                    {link.other?.title ?? otherId}
                  </button>
                  {link.other ? <StatusBadge status={link.other.status} /> : null}
                  {link.auto ? <Badge variant="outline">auto</Badge> : null}
                </dd>
              );
            })}
          </div>
        ))}
      </dl>
    </Section>
  );
}

function Body({ memory, open }: { memory: MemoryView; open: (id: string) => void }) {
  const fields = Object.entries(memory.fields ?? {});
  const evidence = memory.evidence ?? [];
  return (
    <div className="flex flex-col gap-4 px-4 pb-6">
      <p className="text-sm whitespace-pre-wrap [overflow-wrap:anywhere]">{memory.summary}</p>
      {memory.edited_by ? <p className="text-sm text-muted-foreground">Changed by admin</p> : null}
      {memory.reason ? (
        <div>
          <h3 className="text-xs font-medium text-muted-foreground">Why it was saved</h3>
          <p className="text-sm whitespace-pre-wrap [overflow-wrap:anywhere]">{memory.reason}</p>
        </div>
      ) : null}
      {memory.detail ? (
        <details className="flex flex-col gap-2 text-sm">
          <summary className="cursor-pointer font-medium">Full detail</summary>
          <p className="mt-2 whitespace-pre-wrap [overflow-wrap:anywhere]">{memory.detail}</p>
        </details>
      ) : null}
      {fields.length > 0 ? (
        <dl className="flex flex-col gap-2 text-sm">
          {fields.map(([key, value]) => (
            <div key={key}>
              <dt className="text-xs font-medium text-muted-foreground">{words(key)}</dt>
              <dd className="whitespace-pre-wrap [overflow-wrap:anywhere]">
                {Array.isArray(value) ? (
                  <ol className="list-decimal pl-5">
                    {value.map((item, index) => (
                      <li key={index}>{item}</li>
                    ))}
                  </ol>
                ) : (
                  value
                )}
              </dd>
            </div>
          ))}
        </dl>
      ) : null}
      {memory.status === "stale" ? (
        <Section title="Why stale">
          <p className="text-sm [overflow-wrap:anywhere]">
            {memory.missing_entities.length > 0
              ? `${memory.missing_entities.join(", ")} no longer exists${memory.context.branch ? ` on ${memory.context.branch}` : ""}.`
              : memory.stale_cause
                ? memory.stale_cause
                : "Cairn doubts this memory."}
          </p>

        </Section>
      ) : null}
      <Lineage memory={memory} open={open} />
      {evidence.length > 0 || memory.context.commit || memory.context.branch ? (
        <Section title="Evidence">
          <ul className="flex flex-col gap-1 text-sm">
            {evidence.map((item) => (
              <li key={item}>
                <code className="font-mono text-xs [overflow-wrap:anywhere]">{item}</code>
              </li>
            ))}
            {memory.context.branch ? (
              <li>
                Branch <code className="font-mono text-xs">{memory.context.branch}</code>
              </li>
            ) : null}
            {memory.context.commit ? (
              <li>
                Commit <code className="font-mono text-xs">{memory.context.commit.slice(0, 12)}</code>
              </li>
            ) : null}
          </ul>
        </Section>
      ) : null}
      <Section title="Details">
        <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
          <dt className="text-muted-foreground">Trust</dt>
          <dd>{memory.trust}</dd>
          <dt className="text-muted-foreground">Usage</dt>
          <dd className="tabular-nums">
            Fetched {memory.usage.fetched} · helpful {memory.usage.helpful} · wrong {memory.usage.wrong} · detail opened {memory.usage.detail_opened}
          </dd>
          <dt className="text-muted-foreground">Automatic recall</dt>
          <dd className="tabular-nums">pushed {memory.usage.pushed ?? 0}× · used {memory.usage.push_used ?? 0}×</dd>
          <dt className="text-muted-foreground">Last shown</dt>
          <dd><Time value={memory.usage.last_shown_at} /></dd>
          <dt className="text-muted-foreground">Last used</dt>
          <dd><Time value={memory.usage.last_used_at} /></dd>
          <dt className="text-muted-foreground">Created</dt>
          <dd>
            <Time value={memory.created_at} />
            {memory.origin ? ` · saved by ${memory.origin === "system" ? "Cairn" : (memory.device_name ?? "an agent")}` : ""}
          </dd>
          <dt className="text-muted-foreground">Updated</dt>
          <dd>
            <Time value={memory.updated_at} />
          </dd>
          {memory.expires_at ? (
            <Fragment>
              <dt className="text-muted-foreground">Expires</dt>
              <dd>
                <Time value={memory.expires_at} />
              </dd>
            </Fragment>
          ) : null}
        </dl>
        <span className="flex min-w-0 flex-wrap items-center gap-2">
          <code className="min-w-0 font-mono text-xs break-all text-muted-foreground">{memory.id}</code>
          <CopyButton text={memory.id} label="Copy id" onCopied={() => toast.success("Memory id copied")} />
        </span>
      </Section>
    </div>
  );
}

type Load = { id: string; memory: MemoryView | null; error: string | null; missing: boolean };

/**
 * The one memory detail surface. It is driven by `?memory=<id>`, so any page can open it by
 * setting the param and any URL can link to it.
 */
export function MemorySheet() {
  const router = useRouter();
  const account = useAccount();
  const [params, set] = useQueryParams();
  const id = params.get("memory");
  const [loaded, setLoaded] = useState<Load | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [confirming, setConfirming] = useState(false);
  const [pending, setPending] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const editing = id !== null && editingId === id;
  const [title, setTitle] = useState("");
  const [summary, setSummary] = useState("");
  const [reason, setReason] = useState("");
  const [fields, setFields] = useState("");
  const close = useCallback(() => { setEditingId(null); set({ memory: null }); }, [set]);
  useResetOnHide(() => {
    setLoaded(null);
    setConfirming(false);
    setEditingId(null);
  });

  useEffect(() => {
    if (!id) {
      return;
    }
    let cancelled = false;
    cairn<MemoryView>(`memories/${encodeURIComponent(id)}`)
      .then((memory) => !cancelled && setLoaded({ id, memory, error: null, missing: false }))
      .catch((cause: unknown) => {
        if (cancelled) return;
        if (cause instanceof SessionRequiredError) {
          router.replace(LOGIN_PATH);
          return;
        }
        const missing = cause instanceof ApiError && cause.status === 404;
        setLoaded({
          id,
          memory: null,
          missing,
          error: missing ? null : cause instanceof Error ? cause.message : "Could not load this memory.",
        });
      });
    return () => {
      cancelled = true;
    };
  }, [id, attempt, router]);

  // A result for another id is stale: show loading until the current one lands.
  const current = loaded && loaded.id === id ? loaded : null;
  const memory = current?.memory ?? null;
  const canForget = memory?.owner_is_me === true && !memory.project_archived;

  const canPin = canForget && memory !== null && memory.scope !== "global"
    && (memory.status !== "superseded" || memory.pinned_at != null);

  async function togglePin() {
    if (!canPin || !memory) return;
    setPending(true);
    try {
      const result = await cairn<{ pinned_at: number | null }>(`memories/${encodeURIComponent(memory.id)}/pin`, {
        method: "POST",
        body: JSON.stringify({ pinned: memory.pinned_at == null }),
      });
      setLoaded((previous) => previous?.id === memory.id && previous.memory
        ? { ...previous, memory: { ...previous.memory, pinned_at: result.pinned_at } }
        : previous);
      announceMemoriesChanged();
      toast.success(result.pinned_at == null ? "Memory unpinned" : "Memory pinned");
    } catch (cause) {
      if (cause instanceof SessionRequiredError) {
        router.replace(LOGIN_PATH);
        return;
      }
      toast.error(cause instanceof Error ? cause.message : "Could not change the pin.");
    } finally {
      setPending(false);
    }
  }

  async function forget() {
    if (!canForget || !memory) return;
    setPending(true);
    try {
      await cairn("forget", { method: "POST", body: JSON.stringify({ id: memory.id }) });
      toast.success("Memory forgotten");
      announceMemoriesChanged();
      setConfirming(false);
      close();
    } catch (cause) {
      if (cause instanceof SessionRequiredError) {
        router.replace(LOGIN_PATH);
        return;
      }
      toast.error(cause instanceof Error ? cause.message : "Could not forget this memory.");
    } finally {
      setPending(false);
    }
  }

  async function rewrite() {
    if (!id || memory?.project_archived) return;
    setPending(true);
    try {
      const result = await cairn<{memory_id: string}>(`admin/memories/${encodeURIComponent(id)}/rewrite`, { method: "POST", body: JSON.stringify({title, summary, reason, ...(fields.trim() ? {fields: JSON.parse(fields)} : {})}) });
      setEditingId(null);
      announceMemoriesChanged();
      set({ memory: result.memory_id }, "push");
      toast.success("Memory rewritten");
    } catch (cause) { toast.error(cause instanceof Error ? cause.message : "Could not rewrite memory."); }
    finally { setPending(false); }
  }

  return (
    <>
      <Sheet open={id !== null} onOpenChange={(open) => !open && close()}>
        <SheetContent className="overflow-y-auto data-[side=right]:w-full data-[side=right]:sm:max-w-lg">
          <SheetHeader>
            <div className="flex flex-wrap items-center gap-2 pr-8">
              {memory ? (
                <>
                  <KindBadge kind={memory.kind} />
                  {memory.pinned_at != null ? <Badge variant="outline"><Pin aria-hidden="true" />Pinned</Badge> : null}
                  <StatusBadge status={memory.status} cause={memory.stale_cause} />
                  {(() => {
                    const label = memoryLabel({
                      id: memory.id,
                      status: memory.status,
                      stale_cause: memory.stale_cause,
                      wrong: memory.usage.wrong,
                      links: memory.links,
                    });
                    if (!label) return null;
                    return label.id ? (
                      <button
                        type="button"
                        className="rounded-sm text-left text-xs text-muted-foreground underline-offset-4 outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
                        onClick={() => set({ memory: label.id })}
                      >
                        {label.text}
                      </button>
                    ) : (
                      <span className="text-xs text-muted-foreground">{label.text}</span>
                    );
                  })()}
                  <span className="text-xs text-muted-foreground">
                    {memory.project_name ?? words(memory.scope)}
                    {memory.context.branch ? ` · ${memory.context.branch}` : ""}
                  </span>
                </>
              ) : null}
            </div>
            <SheetTitle className="[overflow-wrap:anywhere]">
              {memory?.title ?? (current?.missing ? "Memory not found" : "Memory")}
            </SheetTitle>
            <SheetDescription className="sr-only">Memory details</SheetDescription>
          </SheetHeader>
          {current === null ? <div className="px-4"><LoadingBlock label="Loading memory…" rows={5} /></div> : null}
          {current?.missing ? (
            <div className="flex flex-col items-start gap-3 px-4">
              <p className="text-sm text-muted-foreground">This memory was forgotten.</p>
              {account?.is_admin && !editing ? <Button variant="outline" onClick={() => { setTitle(""); setSummary(""); setReason(""); setFields(""); setEditingId(id); }}>Rewrite by ID…</Button> : null}
              <Button type="button" variant="outline" onClick={close}>
                Close
              </Button>
            </div>
          ) : null}
          {current?.missing && account?.is_admin && editing ? <div className="space-y-3 px-4 pb-4">
            <p className="text-sm">Enter a replacement for this memory.</p>
            <label className="block text-sm">Title<Input value={title} onChange={(event) => setTitle(event.target.value)} /></label>
            <label className="block text-sm">Summary<Textarea value={summary} onChange={(event) => setSummary(event.target.value)} /></label>
            <label className="block text-sm">Reason<Textarea value={reason} onChange={(event) => setReason(event.target.value)} /></label>
            <div className="flex gap-2"><Button disabled={pending} onClick={() => void rewrite()}>Save rewrite</Button><Button variant="outline" onClick={() => setEditingId(null)}>Cancel</Button></div>
          </div> : null}
          {current?.error ? (
            <div className="px-4">
              <ErrorAlert message={current.error} onRetry={() => { setLoaded(null); setAttempt((n) => n + 1); }} />
            </div>
          ) : null}
          {memory ? (
            <>
              <Body memory={memory} open={(next) => set({ memory: next }, "push")} />
              {account?.is_admin && !memory.project_archived && editing ? <div className="space-y-3 px-4 pb-4">
                <label className="block text-sm">Title<Input value={title} onChange={(event) => setTitle(event.target.value)} /></label>
                <label className="block text-sm">Summary<Textarea value={summary} onChange={(event) => setSummary(event.target.value)} /></label>
                <label className="block text-sm">Reason<Textarea value={reason} onChange={(event) => setReason(event.target.value)} /></label>
                <label className="block text-sm">Fields (JSON)<Textarea value={fields} onChange={(event) => setFields(event.target.value)} /></label>
                <div className="flex gap-2"><Button disabled={pending} onClick={() => void rewrite()}>Save rewrite</Button><Button variant="outline" onClick={() => setEditingId(null)}>Cancel</Button></div>
              </div> : account?.is_admin && !memory.project_archived ? <div className="px-4 pb-4"><Button variant="outline" onClick={() => { setTitle(memory.title); setSummary(memory.summary); setReason(memory.reason ?? ""); setFields(JSON.stringify(memory.fields, null, 2)); setEditingId(id); }}>Rewrite memory…</Button></div> : null}
              {canForget ? (
                <div className="mt-auto flex flex-wrap gap-2 px-4 pb-4">
                  {canPin ? (
                    <Button type="button" variant="outline" disabled={pending} onClick={() => void togglePin()}>
                      <Pin aria-hidden="true" />
                      {memory.pinned_at == null ? "Pin memory" : "Unpin memory"}
                    </Button>
                  ) : null}
                  <Button type="button" variant="outline" onClick={() => setConfirming(true)}>
                    Forget memory…
                  </Button>
                </div>
              ) : null}
            </>
          ) : null}
        </SheetContent>
      </Sheet>
      <AlertDialog open={confirming} onOpenChange={(open) => !pending && setConfirming(open)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Forget this memory?</AlertDialogTitle>
            <AlertDialogDescription className="text-left [overflow-wrap:anywhere]">
              {memory ? forgetMemoryImpact(memory.title) : ""}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
            <AlertDialogAction variant="destructive" disabled={pending} onClick={() => void forget()}>
              {pending ? "Working…" : "Forget memory"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
