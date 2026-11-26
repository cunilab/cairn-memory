import type { Metadata } from "next";
import { ConsoleShell } from "@/components/console-shell";
import { PageHeader } from "@/components/page-header";
import { Skills } from "./_components/skills";

export default function SkillsPage() {
  return (
    <ConsoleShell crumbs={[{ label: "Skills" }]}>
      <PageHeader title="Skills" description="Shared procedures and instructions for your agents." />
      <Skills />
    </ConsoleShell>
  );
}

export const metadata: Metadata = { title: "Skills" };
