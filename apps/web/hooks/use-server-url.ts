import { useEffect, useState } from "react";
import { cairn } from "@/lib/cairn";
import type { Health } from "@/lib/generated/Health";

/** The address `cairn setup` is given (`/health` `public_url`); null until known or if unreachable. */
export function useServerUrl(): string | null {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    cairn<Health>("health")
      .then((health) => !cancelled && setUrl(health.public_url))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);
  return url;
}
