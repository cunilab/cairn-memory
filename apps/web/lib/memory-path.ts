/** Every member reads the same live project decisions. */
export function memoryReadPath(projectId: string): string {
  return `projects/${encodeURIComponent(projectId)}/memories`;
}
