"use client";

import { useEffect, useState, type FormEvent } from "react";
import { LoadingBlock } from "@/components/states";
import { Time } from "@/components/time";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Textarea } from "@/components/ui/textarea";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useAccount } from "@/hooks/use-account";
import { cairn } from "@/lib/cairn";
import { useOpenMemory } from "@/hooks/use-query-params";
import type { SkillCard } from "@/lib/generated/SkillCard";
import type { SkillDetail } from "@/lib/generated/SkillDetail";
import type { SkillVersion } from "@/lib/generated/SkillVersion";

export function postSkill<T>(path: string, body?: object): Promise<T> {
  return cairn<T>(`skills/${path}`, { method: "POST", body: body && JSON.stringify(body) });
}

export function SkillSheet({ id, onClose, onChange }: {
  id: string;
  onClose: () => void;
  onChange: (id: string) => void;
}) {
  const account = useAccount();
  const openMemory = useOpenMemory();
  const isNew = id === "new";
  const [skill, setSkill] = useState<SkillDetail>();
  const [history, setHistory] = useState<SkillVersion[]>([]);
  const [version, setVersion] = useState<number>();
  const [editing, setEditing] = useState(isNew);
  const [rollback, setRollback] = useState(false);
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);
  const [error, setError] = useState("");

  useEffect(() => {
    if (isNew) return;
    let active = true;
    const query = version === undefined ? "" : `?version=${version}`;
    Promise.all([cairn<SkillDetail>(`skills/${id}${query}`), cairn<SkillVersion[]>(`skills/${id}/history`)])
      .then(([detail, versions]) => {
        if (active) {
          setSkill(detail);
          setHistory(versions);
        }
      }).catch((failure: Error) => {
        if (active) setError(failure.message);
      });
    return () => { active = false; };
  }, [id, isNew, version, revision]);

  async function mutate(path: string, body?: object, toLatest = false) {
    setBusy(true);
    setError("");
    try {
      const result = await postSkill<SkillCard | null>(path, body);
      if (toLatest) setVersion(undefined);
      setRevision((value) => value + 1);
      setEditing(false);
      setRollback(false);
      onChange(result?.id ?? id);
    } catch (failure) {
      setError((failure as Error).message);
    } finally {
      setBusy(false);
    }
  }

  function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const draft = Object.fromEntries(new FormData(event.currentTarget));
    void mutate(isNew ? "create" : "save", { ...draft, name: skill?.name ?? draft.name }, true);
  }

  const archived = skill?.archived_at != null;
  const hasLoadedVersion = skill && (version === undefined || version === skill.version);
  const historical = skill && history[0]?.version !== skill.version;

  return (
    <Sheet open onOpenChange={(open) => { if (!open) onClose(); }}>
      <SheetContent className="w-full overflow-y-auto sm:max-w-xl">
        <SheetHeader>
          <SheetTitle>{isNew ? "New skill" : skill?.title ?? "Skill"}</SheetTitle>
          {skill && <SheetDescription>{skill.name} · Version {skill.version}</SheetDescription>}
        </SheetHeader>
        <div className="space-y-5 px-4 pb-4">
          {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
          {editing ? (
            <form className="space-y-3" onSubmit={save}>
              {isNew && <div className="space-y-1"><Label htmlFor="skill-name">Name</Label><Input id="skill-name" name="name" required maxLength={64} pattern="[a-z0-9]+(-[a-z0-9]+)*" /></div>}
              <div className="space-y-1"><Label htmlFor="skill-title">Title</Label><Input id="skill-title" name="title" required maxLength={80} defaultValue={skill?.title} /></div>
              <div className="space-y-1"><Label htmlFor="skill-description">Description</Label><Input id="skill-description" name="description" required maxLength={240} defaultValue={skill?.description} /></div>
              <div className="space-y-1"><Label htmlFor="skill-body">Markdown instructions</Label><Textarea id="skill-body" name="body" rows={12} required maxLength={32000} defaultValue={skill?.body} /></div>
              <div className="space-y-1"><Label htmlFor="skill-change">Change note</Label><Input id="skill-change" name="change_note" required maxLength={240} /></div>
              <div className="flex gap-2">
                <Button type="submit" disabled={busy}>{isNew ? "Create global skill" : "Save new version"}</Button>
                <Button type="button" variant="ghost" disabled={busy} onClick={() => isNew ? onClose() : setEditing(false)}>Cancel</Button>
              </div>
            </form>
          ) : hasLoadedVersion ? (
            <>
              <p className="text-sm text-muted-foreground">{skill.description}</p>
              {!historical && skill.source ? (
                <p className="text-sm">
                  Source memory →{" "}
                  <Button type="button" variant="link" className="h-auto p-0" onClick={() => openMemory(skill.source?.memory_id ?? null)}>
                    {skill.source.title}
                  </Button>
                  {skill.source.project_name ? <span className="text-muted-foreground"> · {skill.source.project_name}</span> : null}
                </p>
              ) : null}
              <div className="flex flex-wrap gap-2">
                {!archived && !historical && <Button onClick={() => setEditing(true)} disabled={busy}>Edit</Button>}
                {!archived && historical && <Button onClick={() => setRollback(true)} disabled={busy}>Roll back to this version</Button>}
                {account?.is_admin ? (
                  <Button variant="outline" disabled={busy}
                    onClick={() => void mutate(`${id}/${archived ? "restore" : "archive"}`)}>
                    {archived ? "Restore" : "Archive"}
                  </Button>
                ) : (
                  <Tooltip>
                    <TooltipTrigger render={<span tabIndex={0} />}>
                      <Button variant="outline" disabled>{archived ? "Restore" : "Archive"}</Button>
                    </TooltipTrigger>
                    <TooltipContent>{archived ? "Only admins can restore skills" : "Only admins can archive skills"}</TooltipContent>
                  </Tooltip>
                )}
              </div>
              {rollback && (
                <form className="flex flex-wrap items-end gap-2" onSubmit={(event) => {
                  event.preventDefault();
                  const change_note = new FormData(event.currentTarget).get("change_note");
                  void mutate(`${id}/rollback`, { version: skill.version, change_note }, true);
                }}>
                  <div className="space-y-1"><Label htmlFor="rollback-note">Change note</Label><Input id="rollback-note" name="change_note" maxLength={240} required /></div>
                  <Button type="submit" disabled={busy}>Save rollback as a new version</Button>
                  <Button type="button" variant="ghost" disabled={busy} onClick={() => setRollback(false)}>Cancel</Button>
                </form>
              )}
              <pre className="whitespace-pre-wrap break-words rounded-lg border bg-muted p-3 font-mono text-xs">{skill.body}</pre>
              <form key={skill.version} className="space-y-2" onSubmit={(event) => {
                event.preventDefault();
                const action = (event.nativeEvent as SubmitEvent).submitter as HTMLButtonElement | null;
                if (action) void mutate("feedback", {
                  name_or_id: id, version: skill.version, vote: action.value,
                  note: new FormData(event.currentTarget).get("note"),
                });
              }}>
                {!archived && <div className="space-y-1"><Label htmlFor="skill-vote-note">Vote note (optional)</Label><Input id="skill-vote-note" name="note" defaultValue={skill.my_note ?? ""} maxLength={240} /></div>}
                <div className="flex gap-2">
                  <Button type="submit" variant="outline" value="helpful" aria-pressed={skill.my_vote === "helpful"} disabled={archived || busy}>Helpful · {skill.votes.helpful}</Button>
                  <Button type="submit" variant="outline" value="wrong" aria-pressed={skill.my_vote === "wrong"} disabled={archived || busy}>Wrong · {skill.votes.wrong}</Button>
                </div>
              </form>
            </>
          ) : !error && <LoadingBlock label="Loading skill…" rows={3} />}
          {history.length > 0 && !editing && (
            <section aria-labelledby="skill-history">
              <h2 id="skill-history" className="font-medium">Version history</h2>
              <ul className="mt-2 divide-y rounded-lg border">
                {history.map((item, index) => (
                  <li key={item.version} className="p-3 text-sm">
                    <Button size="sm" variant="link" className="h-auto p-0" aria-current={skill?.version === item.version}
                      onClick={() => { setVersion(index === 0 ? undefined : item.version); setRollback(false); setError(""); }}>
                      Version {item.version}
                    </Button>
                    <p>{item.change_note}</p>
                    <p className="break-all text-xs text-muted-foreground"><Time value={item.created_at} /> · {item.author_id}</p>
                  </li>
                ))}
              </ul>
            </section>
          )}
        </div>
      </SheetContent>
    </Sheet>
  );
}
