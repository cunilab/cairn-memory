/** Counts from `GET /diagnose`. Goes with that route when the dashboard moves to `GET /stats` (7c). */
export type Diagnostics = {
  memories: number;
  by_kind: Record<string, number>;
  by_status: Record<string, number>;
  by_scope: Record<string, number>;
  jobs_pending: number;
  jobs_running: number;
  jobs_quarantined: number;
};
