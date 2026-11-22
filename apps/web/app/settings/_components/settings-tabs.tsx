"use client";

import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useAccount } from "@/hooks/use-account";
import { useTabParam } from "@/hooks/use-query-params";
import { AgentInstructionsEditor } from "@/components/agent-instructions-editor";
import { Accounts } from "./accounts";
import { DataTab } from "./data";
import { Devices } from "./devices";
import { AccountTab } from "./password";

const TABS = ["devices", "account", "data", "instructions", "accounts"] as const;

export function SettingsTabs() {
  const account = useAccount();
  const [tab, setTab] = useTabParam(TABS);
  const admin = account?.is_admin === true;
  return (
    <Tabs value={tab} onValueChange={(value) => setTab(value as (typeof TABS)[number])}>
      <TabsList>
        <TabsTrigger value="devices">Devices</TabsTrigger>
        <TabsTrigger value="account">Account</TabsTrigger>
        <TabsTrigger value="data">Data</TabsTrigger>
        <TabsTrigger value="instructions">Agent instructions</TabsTrigger>
        {admin ? <TabsTrigger value="accounts">Accounts</TabsTrigger> : null}
      </TabsList>
      <TabsContent value="devices">
        <Devices />
      </TabsContent>
      <TabsContent value="account">
        <AccountTab />
      </TabsContent>
      <TabsContent value="data">
        <DataTab />
      </TabsContent>
      <TabsContent value="instructions">
        <AgentInstructionsEditor path="agent-instructions" scope="global" canEdit={admin} />
      </TabsContent>
      {admin || tab === "accounts" ? (
        <TabsContent value="accounts">
          <Accounts />
        </TabsContent>
      ) : null}
    </Tabs>
  );
}
