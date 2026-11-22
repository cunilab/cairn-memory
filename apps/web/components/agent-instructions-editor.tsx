"use client";

import { useCallback, useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { toast } from "sonner";
import { ErrorAlert, LoadingBlock } from "@/components/states";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Textarea } from "@/components/ui/textarea";
import { SOURCE_LABEL, byteCounter, changedFields, overLimit, sampleStart, sampleStop } from "@/lib/agent-instructions";
import { cairn } from "@/lib/cairn";
import type { AgentInstructions } from "@/lib/generated/AgentInstructions";
import type { InstructionField } from "@/lib/generated/InstructionField";
import { LOGIN_PATH, SessionRequiredError } from "@/lib/session";

type Key = "session_instruction" | "stop_nudge";

const COPY: Record<Key, { title: string; help: string; preview: (text: string) => string; where: string }> = {
  session_instruction: {
    title: "Session start line",
    help: "One instruction added to the session brief when an agent starts in a repository with .cairn/project.json.",
    preview: sampleStart,
    where: "Added after the brief",
  },
  stop_nudge: {
    title: "Stop reminder",
    help: "Sent once, when a session changed files or ran tests and saved nothing to Cairn. {files} becomes the number of edited files.",
    preview: (text) => sampleStop(text),
    where: "Shown to the agent at Stop (with 3 edited files here)",
  },
};

/**
 * The two agent instruction texts at one scope: the global ones (`agent-instructions`) or a
 * project's overrides (`projects/<id>/agent-instructions`). Everyone reads; `canEdit` is only what
 * the server will accept, and the server checks it again.
 */
export function AgentInstructionsEditor({
  path,
  scope,
  canEdit,
}: {
  path: string;
  scope: "global" | "project";
  canEdit: boolean;
}) {
  const router = useRouter();
  const [data, setData] = useState<AgentInstructions | null>(null);
  const [draft, setDraft] = useState<Record<Key, string>>({ session_instruction: "", stop_nudge: "" });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [attempt, setAttempt] = useState(0);

  const adopt = useCallback((next: AgentInstructions) => {
    setData(next);
    setDraft({ session_instruction: next.session_instruction.own ?? "", stop_nudge: next.stop_nudge.own ?? "" });
  }, []);

  useEffect(() => {
    let cancelled = false;
    cairn<AgentInstructions>(path)
      .then((next) => {
        if (!cancelled) adopt(next);
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        if (cause instanceof SessionRequiredError) router.replace(LOGIN_PATH);
        else setError(cause instanceof Error ? cause.message : "Could not load the agent instructions.");
      });
    return () => {
      cancelled = true;
    };
  }, [path, attempt, adopt, router]);

  async function save(body: Partial<Record<Key, string>>, done: string) {
    setBusy(true);
    setError(null);
    try {
      adopt(await cairn<AgentInstructions>(path, { method: "POST", body: JSON.stringify(body) }));
      toast.success(done);
    } catch (cause) {
      if (cause instanceof SessionRequiredError) router.replace(LOGIN_PATH);
      else setError(cause instanceof Error ? cause.message : "Could not save.");
    } finally {
      setBusy(false);
    }
  }

  if (error && !data) return <ErrorAlert message={error} onRetry={() => {
          setError(null);
          setAttempt((n) => n + 1);
        }} />;
  if (!data) return <LoadingBlock label="Loading agent instructions" rows={3} />;

  const changes = changedFields(draft, {
    session_instruction: data.session_instruction.own,
    stop_nudge: data.stop_nudge.own,
  });
  const invalid = (Object.keys(draft) as Key[]).some((key) => overLimit(draft[key], data.max_bytes));
  const clearLabel = scope === "project" ? "Clear override" : "Reset to default";

  return (
    <div className="flex max-w-2xl flex-col gap-6">
      <p className="text-sm text-muted-foreground">
        {scope === "project"
          ? "These apply to agents working in this project. A field you leave empty uses the global text, then the built-in default."
          : "These apply to every project unless a project sets its own. A field you leave empty uses the built-in default."}{" "}
        {canEdit
          ? ""
          : scope === "project"
            ? "Only the project's owner or a server admin can change them."
            : "Only a server admin can change them."}
      </p>
      {(Object.keys(COPY) as Key[]).map((key) => {
        const field: InstructionField = data[key];
        const copy = COPY[key];
        const typed = draft[key].trim();
        const shown = typed || field.inherited;
        const id = `${scope}-${key}`;
        return (
          <section key={key} className="flex flex-col gap-3">
            <Field>
              <div className="flex flex-wrap items-center gap-2">
                <FieldLabel htmlFor={id}>{copy.title}</FieldLabel>
                <Badge variant="secondary">Using {SOURCE_LABEL[field.source]}</Badge>
              </div>
              <FieldDescription>{copy.help}</FieldDescription>
              <Textarea
                id={id}
                value={draft[key]}
                placeholder={field.inherited}
                disabled={!canEdit || busy}
                aria-invalid={overLimit(draft[key], data.max_bytes)}
                aria-describedby={`${id}-count`}
                rows={4}
                onChange={(event) => setDraft((current) => ({ ...current, [key]: event.target.value }))}
              />
              <p
                id={`${id}-count`}
                className={`text-xs tabular-nums ${overLimit(draft[key], data.max_bytes) ? "text-destructive" : "text-muted-foreground"}`}
              >
                {byteCounter(draft[key], data.max_bytes)}
              </p>
            </Field>
            <div className="flex flex-col gap-1">
              <p className="text-xs font-medium text-muted-foreground">Preview: {copy.where}</p>
              <pre className="whitespace-pre-wrap rounded-lg border bg-muted/40 p-3 text-sm [overflow-wrap:anywhere]">
                {copy.preview(shown)}
              </pre>
            </div>
            {canEdit && field.own ? (
              <div>
                <Button type="button" variant="outline" size="sm" disabled={busy} onClick={() => void save({ [key]: "" }, `${copy.title} reset`)}>
                  {clearLabel}
                </Button>
              </div>
            ) : null}
          </section>
        );
      })}
      {error ? <ErrorAlert message={error} /> : null}
      {canEdit ? (
        <div>
          <Button
            type="button"
            disabled={busy || invalid || Object.keys(changes).length === 0}
            onClick={() => void save(changes, "Agent instructions saved")}
          >
            Save
          </Button>
        </div>
      ) : null}
    </div>
  );
}
