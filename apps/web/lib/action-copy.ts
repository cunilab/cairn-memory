export function revokeDeviceImpact(name: string): string {
  return `Revoke ${name}. Its token stops working at once, so any agent using it can no longer read or write memory. Revoking cannot be undone; create a new device to reconnect an agent. The memories it already captured stay on your account.`;
}

export function rotateDeviceImpact(name: string): string {
  return `Rotate the token for ${name}. The old token stops working at once, so any agent still using it can no longer read or write memory until you give it the new token. The new token is shown once and cannot be retrieved later.`;
}

export function forgetMemoryImpact(title: string): string {
  return `Forget '${title}'? Agents will no longer see it. This cannot be undone.`;
}
