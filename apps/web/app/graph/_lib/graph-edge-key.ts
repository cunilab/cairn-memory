/** Length-prefixed so a memory id that contains ":" cannot collide with another triple. */
export function graphEdgeKey(source: string, relation: string, target: string): string {
  return `${source.length}:${source}${relation.length}:${relation}${target.length}:${target}`;
}
