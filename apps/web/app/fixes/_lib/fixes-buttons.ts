type Fix = { fix_type: "server" | "computer" | "agent" | "report"; status: "pending" | "applying" | "queued" | "waiting" | "done" | "failed" | "resolved" | "ignored" | "expired" };

const ISSUES: Record<string, string> = {
  F1: "Review a project with no saves",
  F2: "Review a stale memory",
  F3: "Review a memory whose files are gone",
  F4: "Reconnect a device",
  F5: "Update an agent",
  F6: "Review project membership",
  F7: "Review a merged memory",
  F8: "Review search setup",
  F9: "Review possible secret exposure",
  F10: "Review a noisy project",
  F11: "Review a repeated phrase",
  F12: "Review an unused memory",
};

export function issueTitle(detector: string): string {
  return ISSUES[detector] ?? detector;
}

export function buttonsFor(fix: Fix): string[] {
  if (fix.status === "pending") {
    if (fix.fix_type === "agent") return ["Copy prompt", "Mark done", "Ignore"];
    if (fix.fix_type === "report") return ["Ignore"];
    return ["Apply", "Ignore"];
  }
  if (fix.status === "failed") return ["Apply", "Ignore"];
  if (fix.status === "waiting" && fix.fix_type === "agent") return ["Copy prompt", "Mark done", "Ignore"];
  if (fix.status === "queued" && fix.fix_type === "computer") return ["Ignore"];
  return [];
}
