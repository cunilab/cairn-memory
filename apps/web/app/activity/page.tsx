import { Suspense } from "react";
import type { Metadata } from "next";
import { ConsoleShell } from "@/components/console-shell";
import { PageHeader } from "@/components/page-header";
import { LoadingBlock } from "@/components/states";
import { ActivityTabs } from "./_components/activity-tabs";

export default function ActivityPage() {
  return (
    <ConsoleShell crumbs={[{ label: "Activity" }]}>
      <PageHeader
        title="Activity"
        description="What Cairn and your agents did on their own, plus an audit trail of sign-ins and changes."
      />
      <Suspense fallback={<LoadingBlock label="Loading activity…" rows={6} />}>
        <ActivityTabs />
      </Suspense>
    </ConsoleShell>
  );
}

export const metadata: Metadata = { title: "Activity" };
