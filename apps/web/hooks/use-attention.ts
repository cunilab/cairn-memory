import { useEffect, useState } from "react";
import { cairn } from "@/lib/cairn";
import type { Stats } from "@/lib/generated/Stats";

const REFRESH_MS = 60_000;

/** Sidebar dots by route: Settings when a device needs an update, Activity when a job stopped retrying. */
export function useAttention(): Record<string, string> {
  const [attention, setAttention] = useState<Record<string, string>>({});
  useEffect(() => {
    let cancelled = false;
    const load = () =>
      cairn<Stats>("stats?days=7")
        .then((stats) => {
          if (cancelled) return;
          setAttention({
            ...(stats.devices_attention.length > 0 ? { "/settings": "A device needs attention" } : {}),
            ...(stats.jobs.quarantined > 0 ? { "/activity": "A job stopped retrying" } : {}),
          });
        })
        .catch(() => {});
    void load();
    const timer = setInterval(() => document.visibilityState === "visible" && void load(), REFRESH_MS);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, []);
  return attention;
}
