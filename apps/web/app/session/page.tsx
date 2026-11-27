import { Suspense } from "react";
import type { Metadata } from "next";
import { ConsoleShell } from "@/components/console-shell";
import { LoadingBlock } from "@/components/states";
import { SessionInspector } from "./inspector";

export default function SessionPage() {
  return (
    <ConsoleShell crumbs={[{ label: "Session" }]}>
      <Suspense fallback={<LoadingBlock label="Loading session…" rows={6} />}>
        <SessionInspector />
      </Suspense>
    </ConsoleShell>
  );
}

export const metadata: Metadata = { title: "Session" };
