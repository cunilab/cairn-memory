"use client";

import { useState } from "react";
import Link from "next/link";
import { CheckIcon, CircleDashedIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { SetupCommand } from "@/components/setup-command";
import { Spinner } from "@/components/ui/spinner";
import { useServerUrl } from "@/hooks/use-server-url";
import type { Agent } from "@/lib/setup-command";

type Props = {
  /** A device has checked in. */
  checkedIn: boolean;
  /** Something has been captured. */
  captured: boolean;
  total: number;
  /** The wait ran out; point at `cairn doctor`. */
  gaveUp: boolean;
};

function Status({ done, children }: { done: boolean; children: React.ReactNode }) {
  return (
    <span className="flex items-center gap-2 text-sm">
      {done ? <CheckIcon className="size-4 text-success" aria-hidden /> : <CircleDashedIcon className="size-4 text-muted-foreground" aria-hidden />}
      {children}
    </span>
  );
}

/** First run: a checklist of real controls. It disappears once the first capture arrives. */
export function FirstRun({ checkedIn, captured, total, gaveUp }: Props) {
  const done = 1 + (checkedIn ? 1 : 0) + (captured ? 1 : 0);
  const serverUrl = useServerUrl();
  const [agent, setAgent] = useState<Agent>("claude");
  return (
    <Card>
      <CardHeader>
        <CardTitle>
          <h2>Connect your first agent</h2>
        </CardTitle>
        <CardDescription>{done} of 3 done</CardDescription>
      </CardHeader>
      <CardContent>
        <ol aria-live="polite" className="flex flex-col gap-4">
          <li>
            <Status done>Signed in</Status>
          </li>
          <li className="flex flex-col gap-2">
            <Status done={checkedIn}>Create a device token and run the setup command</Status>
            <div className="flex flex-wrap items-center gap-2 pl-6">
              <Button nativeButton={false} render={<Link href="/settings?tab=devices&create=1" />}>
                Create device token
              </Button>
              <span className="text-sm text-muted-foreground">Then run this on the machine where your agent runs.</span>
            </div>
            <div className="pl-6">
              <SetupCommand serverUrl={serverUrl} agent={agent} onAgent={setAgent} />
            </div>
          </li>
          <li className="flex flex-col gap-2">
            <Status done={captured}>Start an agent session</Status>
            <p className="flex items-center gap-2 pl-6 text-sm text-muted-foreground">
              {captured ? (
                <>
                  First memories arrived: {total.toLocaleString()} saved.
                  <Button nativeButton={false} variant="link" render={<Link href="/memories" />}>
                    See memories
                  </Button>
                </>
              ) : gaveUp ? (
                "Still waiting. Run cairn doctor on that machine to see what is wrong."
              ) : (
                <>
                  <Spinner aria-hidden />
                  {checkedIn ? "Waiting for the first capture…" : "Waiting for the first check-in…"}
                </>
              )}
            </p>
          </li>
        </ol>
      </CardContent>
    </Card>
  );
}
