"use client";

import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useTabParam } from "@/hooks/use-query-params";
import { AuditTable } from "./audit-table";
import { Jobs } from "./jobs";
import { Timeline } from "./timeline";

const TABS = ["timeline", "audit", "jobs"] as const;

export function ActivityTabs() {
  const [tab, setTab] = useTabParam(TABS);
  return (
    <Tabs value={tab} onValueChange={(value) => setTab(value as (typeof TABS)[number])}>
      <TabsList>
        <TabsTrigger value="timeline">Timeline</TabsTrigger>
        <TabsTrigger value="audit">Audit log</TabsTrigger>
        <TabsTrigger value="jobs">Jobs</TabsTrigger>
      </TabsList>
      <TabsContent value="timeline">
        <Timeline />
      </TabsContent>
      <TabsContent value="audit">
        <AuditTable />
      </TabsContent>
      <TabsContent value="jobs">
        <Jobs />
      </TabsContent>
    </Tabs>
  );
}
