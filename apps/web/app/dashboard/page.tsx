import type { Metadata } from "next";
import { Overview } from "@/components/overview";

export default function DashboardPage() {
  return <Overview />;
}

export const metadata: Metadata = { title: "Overview" };
