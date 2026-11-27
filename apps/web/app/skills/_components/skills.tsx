"use client";

import { useEffect, useState } from "react";
import type { ColumnDef } from "@tanstack/react-table";
import { DataTable } from "@/components/data-table";
import { LoadingBlock } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useQueryParams, useTabParam } from "@/hooks/use-query-params";
import { cairn } from "@/lib/cairn";
import type { SkillCard } from "@/lib/generated/SkillCard";
import type { SkillSearchResults } from "@/lib/generated/SkillSearchResults";
import type { SkillSuggestion } from "@/lib/generated/SkillSuggestion";
import { SkillSheet, postSkill } from "./skill-sheet";

export function Skills() {
  const [query, setQuery] = useState("");
  const [tab, setTab] = useTabParam(["library", "suggestions", "archived"] as const);
  const archived = tab === "archived";
  const [params, setParams] = useQueryParams();
  const [offset, setOffset] = useState(0);
  const [data, setData] = useState<SkillSearchResults>();
  const [suggestions, setSuggestions] = useState<SkillSuggestion[]>([]);
  const [selected, setSelected] = useState<string | null>(params.get("skill"));
  const [revision, setRevision] = useState(0);
  const [error, setError] = useState("");
  const [converting, setConverting] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    const params = new URLSearchParams({ query, archived: String(archived), offset: String(offset) });
    cairn<SkillSearchResults>(`skills?${params}`).then((value) => {
      if (active) setData(value);
    }).catch((failure: Error) => {
      if (active) setError(failure.message);
    });
    return () => { active = false; };
  }, [query, archived, offset, revision]);

  useEffect(() => {
    let active = true;
    cairn<SkillSuggestion[]>("skills/suggestions").then((value) => {
      if (active) setSuggestions(value);
    }).catch((failure: Error) => {
      if (active) setError(failure.message);
    });
    return () => { active = false; };
  }, [revision]);

  function refresh(id: string) {
    setSelected(id);
    setRevision((value) => value + 1);
  }

  async function resolve(suggestion: SkillSuggestion, action: "create" | "dismiss") {
    setConverting(suggestion.memory_id);
    setError("");
    try {
      const result = await postSkill<SkillCard | null>(`suggestions/${suggestion.memory_id}/${action}`);
      setSuggestions((items) => items.filter((item) => item.memory_id !== suggestion.memory_id));
      if (result) refresh(result.id);
    } catch (failure) {
      setError((failure as Error).message);
    } finally {
      setConverting(null);
    }
  }

  const columns: ColumnDef<SkillCard>[] = [
    {
      accessorKey: "title",
      header: "Skill",
      cell: ({ row }) => (
        <Button variant="link" className="h-auto whitespace-normal p-0 text-left" onClick={() => { setSelected(row.original.id); setParams({ skill: row.original.id }, "push"); }}>
          {row.original.title}
        </Button>
      ),
    },
    { accessorKey: "description", header: "Description" },
    { accessorKey: "version", header: "Version" },
    { id: "votes", header: "Votes", cell: ({ row }) => <>{row.original.votes.helpful} helpful · {row.original.votes.wrong} wrong</> },
  ];

  return (
    <div className="space-y-4">
      <Tabs value={tab} onValueChange={(value) => { setOffset(0); setTab(value as "library" | "suggestions" | "archived"); }}>
        <TabsList>
          <TabsTrigger value="library">Library</TabsTrigger>
          <TabsTrigger value="suggestions">Suggestions{suggestions.length > 0 ? ` ${suggestions.length}` : ""}</TabsTrigger>
          <TabsTrigger value="archived">Archived</TabsTrigger>
        </TabsList>
      </Tabs>
      {tab !== "suggestions" ? (
      <form className="flex flex-wrap items-center gap-2" onSubmit={(event) => {
        event.preventDefault();
        setError("");
        setOffset(0);
        setQuery(String(new FormData(event.currentTarget).get("query")).trim());
      }}>
        <Input name="query" type="search" aria-label="Search skills" placeholder="Search skills" className="w-full sm:max-w-xs" />
        <Button type="submit" variant="outline">Search</Button>
        <Button type="button" onClick={() => setSelected("new")}>New skill</Button>
      </form>
      ) : null}
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      {tab === "suggestions" ? (
        suggestions.length > 0 ? (
        <ul className="divide-y rounded-lg border bg-card">
          {suggestions.map((suggestion) => (
            <li key={suggestion.memory_id} className="flex flex-wrap items-center justify-between gap-3 p-3">
              <div className="min-w-0">
                <p className="text-sm font-medium">{suggestion.title}</p>
                <p className="text-sm text-muted-foreground">{suggestion.description}</p>
              </div>
              <div className="flex gap-2">
                <Button size="sm" disabled={converting !== null} onClick={() => void resolve(suggestion, "create")}>Create global skill</Button>
                <Button size="sm" variant="ghost" disabled={converting !== null} onClick={() => void resolve(suggestion, "dismiss")}>Dismiss</Button>
              </div>
            </li>
          ))}
        </ul>
        ) : <p className="text-sm text-muted-foreground">No suggestions.</p>
      ) : data ? (
        <>
          <DataTable label="skills" filterPlaceholder="" searchable={false} columns={columns} data={data.results}
            getRowId={(row) => row.id} pageSize={20}
            empty={<p className="p-6 text-sm text-muted-foreground">{query ? "No skills match." : archived ? "No archived skills." : "No skills yet."}</p>} />
          {(offset > 0 || data.next_offset !== null) && (
            <div className="flex gap-2">
              <Button variant="outline" disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - 20))}>Previous</Button>
              <Button variant="outline" disabled={data.next_offset === null} onClick={() => setOffset(data.next_offset ?? offset)}>Next</Button>
            </div>
          )}
        </>
      ) : !error && <LoadingBlock label="Loading skills…" rows={4} />}
      {selected && <SkillSheet key={selected} id={selected} onClose={() => setSelected(null)} onChange={refresh} />}
    </div>
  );
}
