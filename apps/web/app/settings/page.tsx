import { Suspense } from "react";
import type { Metadata } from "next";
import { ConsoleShell } from "@/components/console-shell";
import { PageHeader } from "@/components/page-header";
import { LoadingBlock } from "@/components/states";
import { SettingsTabs } from "./_components/settings-tabs";

export default function SettingsPage() {
  return (
    <ConsoleShell crumbs={[{ label: "Settings" }]}>
      <PageHeader
        title="Settings"
        description="Connect agents, manage access and move your data."
      />
      <Suspense fallback={<LoadingBlock label="Loading settings…" rows={4} />}>
        <SettingsTabs />
      </Suspense>
    </ConsoleShell>
  );
}

export const metadata: Metadata = { title: "Settings" };
