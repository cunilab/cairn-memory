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
