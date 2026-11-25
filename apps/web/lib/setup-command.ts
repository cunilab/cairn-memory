export const AGENTS = [
  { value: "claude", label: "Claude" },
  { value: "codex", label: "Codex" },
  { value: "opencode", label: "OpenCode" },
  { value: "grok", label: "Grok Build" },
  { value: "antigravity", label: "Antigravity CLI" },
  { value: "hermes", label: "Hermes Agent" },
] as const;
export type Agent = (typeof AGENTS)[number]["value"];

/** The command a person runs on the agent's machine. `serverUrl` is the server's `public_url`. */
export function setupCommand(serverUrl: string | null, agent: Agent): string {
  const url = serverUrl?.trim().replace(/\/+$/, "");
  return `cairn setup ${url || "<server-url>"} --agent ${agent}`;
}
