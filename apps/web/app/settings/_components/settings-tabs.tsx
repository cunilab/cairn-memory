"use client";

import { useAccount } from "@/hooks/use-account";
import { useTabParam } from "@/hooks/use-query-params";
import { AgentInstructionsEditor } from "@/components/agent-instructions-editor";
import { Accounts } from "./accounts";
import { DataTab } from "./data";
import { Devices } from "./devices";
import { AccountTab } from "./password";

const TABS = ["devices", "account", "data", "instructions", "accounts"] as const;
const ACCOUNT = [
  ["devices", "Devices"],
  ["account", "Profile"],
  ["data", "Data"],
] as const;
const ADMIN = [
  ["accounts", "Accounts"],
  ["instructions", "Agent instructions"],
] as const;

export function SettingsTabs() {
  const account = useAccount();
  const [tab, setTab] = useTabParam(TABS);
  const admin = account?.is_admin === true;
  const sections = admin ? [...ACCOUNT, ...ADMIN] : [...ACCOUNT, ["instructions", "Agent instructions"] as const];
  return (
    <div className="grid gap-4 md:grid-cols-[13rem_minmax(0,1fr)]">
      <nav aria-label="Settings sections" className="flex flex-col gap-3">
        <label className="grid gap-1 text-sm md:hidden">
          Section
          <select className="h-11 rounded-md border bg-background px-2" value={sections.some((item) => item[0] === tab) ? tab : "devices"} onChange={(event) => setTab(event.target.value as (typeof TABS)[number])}>
            <optgroup label="Your account">
              {ACCOUNT.map(([value, label]) => <option key={value} value={value}>{label}</option>)}
            </optgroup>
            <optgroup label="Administration">
              {(admin ? ADMIN : [["instructions", "Agent instructions"] as const]).map(([value, label]) => <option key={value} value={value}>{label}</option>)}
            </optgroup>
          </select>
        </label>
        <div className="hidden flex-col gap-3 md:flex">
          <p className="text-xs font-medium text-muted-foreground">Your account</p>
          {ACCOUNT.map(([value, label]) => (
            <button key={value} type="button" aria-current={tab === value ? "page" : undefined} className="rounded-md px-2 py-1.5 text-left text-sm aria-[current=page]:bg-muted" onClick={() => setTab(value)}>
              {label}
            </button>
          ))}
          <p className="text-xs font-medium text-muted-foreground">Administration</p>
          {(admin ? ADMIN : [["instructions", "Agent instructions"] as const]).map(([value, label]) => (
            <button key={value} type="button" aria-current={tab === value ? "page" : undefined} className="rounded-md px-2 py-1.5 text-left text-sm aria-[current=page]:bg-muted" onClick={() => setTab(value)}>
              {label}
            </button>
          ))}
        </div>
      </nav>
      <div className="min-w-0">
        {tab === "devices" ? <Devices /> : null}
        {tab === "account" ? <AccountTab /> : null}
        {tab === "data" ? <DataTab /> : null}
        {tab === "instructions" ? <AgentInstructionsEditor path="agent-instructions" scope="global" canEdit={admin} /> : null}
        {tab === "accounts" && admin ? <Accounts /> : null}
      </div>
    </div>
  );
}
