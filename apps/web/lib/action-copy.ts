export function deleteDecisionImpact(memoryId: string): string {
  return `This hides ${memoryId} from the project. Cairn keeps the stored record and records a retained delete. The decision leaves this page and every member’s project view. Your private source stays as it is. This console cannot undo it.`;
}

export function revokeDeviceImpact(name: string): string {
  return `Revoke ${name}. Its token stops working at once, so any agent using it can no longer read or write memory. Revoking cannot be undone; create a new device to reconnect an agent. The memories it already captured stay on your account.`;
}

export function rotateDeviceImpact(name: string): string {
  return `Rotate the token for ${name}. The old token stops working at once, so any agent still using it can no longer read or write memory until you give it the new token. The new token is shown once and cannot be retrieved later.`;
}

export function closeBranchImpact(name: string): string {
  return `Close your branch ${name}. Your personal memories stay on your account, and other members’ branch records stay as they are.`;
}

export function mergeBranchImpact(name: string): string {
  return `Record the merge of your branch ${name}. The selected memory becomes a decision for every member; only your branch and worktree records change. Preview the full copy first.`;
}
