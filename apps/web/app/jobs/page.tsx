import type { Metadata } from "next";
import { Jobs } from "@/components/jobs";

export default function JobsPage() {
  return <Jobs />;
}

export const metadata: Metadata = { title: "Jobs" };
