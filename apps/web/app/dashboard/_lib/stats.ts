import { cairn } from "@/lib/cairn";
import type { Device, Diagnostics, ListedProject } from "@/lib/types";
import type { UsageDays } from "./summary";

/**
 * Everything the dashboard shows. The first block is available today; the optional block is
 * what step 7a adds to `GET /stats` (UX spec §5). Sections render only when their field is
 * present.
 */
export interface DashboardStats {
  memories: { total: number; by_status: Record<string, number>; by_kind: Record<string, number> };
  projects: number;
  jobs: { pending: number; running: number; quarantined: number };
  /** No device has ever checked in, or nothing has been captured yet. */
  first_run: boolean;
  devices_checked_in: number;

  // TODO(7a): replace this loader with `GET /stats?tz=&days=` and read these from it.
  usage_days?: UsageDays;
  bench?: { hit_at_5: number; search_tokens: number; get_tokens: number; session_tokens: number };
  usage?: {
    fetched_30d: number;
    helpful_30d: number;
    never_fetched: number;
    top: { id: string; kind: string; title: string; fetched: number }[];
  };
  memory_health?: { expiring: number; stale_missing: number; stale_feedback: number; linked_ratio: number };
  devices_attention?: { device_id: string; name: string; reason: string }[];
  capture?: { last_at?: number | null; observations_7d: number };
}

/**
 * TODO(7a): today this joins three small calls (`diagnose`, `projects`, `devices`) instead of
 * downloading every memory. `GET /stats` replaces all of it.
 */
export async function loadStats(): Promise<DashboardStats> {
  const [diagnostics, projects, devices] = await Promise.all([
    cairn<Diagnostics>("diagnose"),
    cairn<ListedProject[]>("projects"),
    cairn<Device[]>("devices"),
  ]);
  const checkedIn = devices.filter((d) => d.status !== "revoked" && d.last_seen_at !== d.created_at).length;
  return {
    memories: { total: diagnostics.memories, by_status: diagnostics.by_status, by_kind: diagnostics.by_kind },
    projects: projects.length,
    jobs: { pending: diagnostics.jobs_pending, running: diagnostics.jobs_running, quarantined: diagnostics.jobs_quarantined },
    devices_checked_in: checkedIn,
    first_run: checkedIn === 0 || diagnostics.memories === 0,
  };
}
