"use client";

import { CopyButton } from "@/components/copy-button";
import { AGENTS, type Agent, setupCommand } from "@/lib/setup-command";

/** The agent choice, the `cairn setup <url>` command and a copy button. */
export function SetupCommand({
  serverUrl,
  agent,
  onAgent,
}: {
  serverUrl: string | null;
  agent: Agent;
  onAgent: (agent: Agent) => void;
}) {
  const command = setupCommand(serverUrl, agent);
  return (
    <div className="flex flex-col gap-2">
      <fieldset className="flex flex-wrap gap-x-4 gap-y-1">
        <legend className="sr-only">Agent</legend>
        {AGENTS.map((item) => (
          <label key={item.value} className="flex min-h-8 items-center gap-2 text-sm">
            <input
              type="radio"
              name="agent"
              value={item.value}
              checked={agent === item.value}
              onChange={() => onAgent(item.value)}
            />
            {item.label}
          </label>
        ))}
      </fieldset>
      <div className="flex flex-wrap items-center gap-2">
        <code className="min-w-0 flex-1 rounded-md border bg-muted p-3 font-mono text-sm break-all">{command}</code>
        <CopyButton text={command} label="Copy" />
      </div>
      {serverUrl === null ? (
        <p className="text-sm text-muted-foreground">Replace <code>&lt;server-url&gt;</code> with this server&apos;s address.</p>
      ) : null}
    </div>
  );
}
