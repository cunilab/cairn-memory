export function deleteDecisionImpact(memoryId: string): string {
  return `This hides ${memoryId} from the project. Cairn keeps the stored record and records a retained delete. The decision leaves this page and the member shared view. This console cannot undo it.`;
}

export function revokeShareImpact(memoryId: string, email: string): string {
  const member = email.trim();
  if (!member) {
    return "Enter a member email before revoking this share. No request is sent until you confirm.";
  }
  return `Stop sharing ${memoryId} with ${member}. The decision stays on your account. ${member} will no longer see it in the shared project. Share this decision again to show it to this member.`;
}

export function revokeDeviceImpact(name: string): string {
  return `Revoke ${name}. Its token stops working at once, so any agent using it can no longer read or write memory. Revoking cannot be undone; create a new device to reconnect an agent. The memories it already captured stay on your account.`;
}

export function rotateDeviceImpact(name: string): string {
  return `Rotate the token for ${name}. The old token stops working at once, so any agent still using it can no longer read or write memory until you give it the new token. The new token is shown once and cannot be retrieved later.`;
}

export function closeBranchImpact(name: string): string {
  return `Close branch ${name}. A closed branch cannot be merged into the project and this console cannot reopen it. Memories captured on the branch stay on your account.`;
}

export function mergeBranchImpact(name: string): string {
  return `Merge branch ${name}. The chosen memory becomes a project decision, the branch is marked merged, and this console cannot undo it.`;
}

export function sharePolicyImpact(policy: string): string {
  return policy === "manual"
    ? "Set the share policy to manual. New decisions stay private until you share each one with a member."
    : "Set the share policy to automatic. New non-sensitive project decisions are shared with project members without a per-decision step.";
}
