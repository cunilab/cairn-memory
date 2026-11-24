import { Suspense } from "react";
import type { Metadata } from "next";
import { ConsoleShell } from "@/components/console-shell";
import { PageHeader } from "@/components/page-header";
import { LoadingBlock } from "@/components/states";
import { Fixes } from "./_components/fixes";

export default function FixesPage() {
  return <ConsoleShell crumbs={[{ label: "Fixes" }]}><PageHeader title="Fixes" description="Review problems and choose what to change." /><Suspense fallback={<LoadingBlock label="Loading fixes…" rows={5} />}><Fixes /></Suspense></ConsoleShell>;
}
export const metadata: Metadata = { title: "Fixes" };
