/** Owner reads project decisions. Every other role reads only the shared list. */
export function memoryReadPath(projectId: string, role: string): string {
  const id = encodeURIComponent(projectId);
  if (role === "owner") {
    return `projects/${id}/memories`;
  }
  return `projects/${id}/shared`;
}
