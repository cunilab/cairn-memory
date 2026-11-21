import { Suspense } from "react";
import type { Metadata } from "next";
import { ConsoleShell } from "@/components/console-shell";
import { PageHeader } from "@/components/page-header";
import { LoadingBlock } from "@/components/states";
import { Dashboard } from "./_components/dashboard";

export default function DashboardPage() {
  return (
    <ConsoleShell crumbs={[{ label: "Dashboard" }]}>
      <PageHeader title="Dashboard" description="Is Cairn working, and is it worth it?" />
      <Suspense fallback={<LoadingBlock label="Loading dashboard…" rows={5} />}>
        <Dashboard />
      </Suspense>
    </ConsoleShell>
  );
}

export const metadata: Metadata = { title: "Dashboard" };
