export function deleteDecisionImpact(memoryId: string): string {
  return `This forgets ${memoryId}. Its text is erased for every member and Cairn will not accept the same memory again. This console cannot undo it.`;
}

export function revokeDeviceImpact(name: string): string {
  return `Revoke ${name}. Its token stops working at once, so any agent using it can no longer read or write memory. Revoking cannot be undone; create a new device to reconnect an agent. The memories it already captured stay on your account.`;
}

export function rotateDeviceImpact(name: string): string {
  return `Rotate the token for ${name}. The old token stops working at once, so any agent still using it can no longer read or write memory until you give it the new token. The new token is shown once and cannot be retrieved later.`;
}
