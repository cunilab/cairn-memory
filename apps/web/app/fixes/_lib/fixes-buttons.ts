type Fix = { fix_type: "server" | "computer" | "agent" | "report"; status: "pending" | "applying" | "queued" | "waiting" | "done" | "failed" | "resolved" | "ignored" | "expired" };

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
