const ABSENT_TEAM = "validation error: project has no team space";

export function isAbsentTeam(message: string): boolean {
  return message === ABSENT_TEAM;
}
