import type { Metadata } from "next";
import { Accounts } from "@/components/accounts";

export default function AccountsPage() {
  return <Accounts />;
}

export const metadata: Metadata = { title: "Accounts" };
