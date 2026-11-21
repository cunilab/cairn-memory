import { useCallback } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";

type Patch = Record<string, string | null | undefined>;

/**
 * The URL's search params and a setter. Empty values drop the key. Filters and tabs replace
 * the history entry; opening a detail sheet pushes one, so Back closes it.
 * Callers must sit under a Suspense boundary (useSearchParams).
 */
export function useQueryParams() {
  const router = useRouter();
  const pathname = usePathname();
  const params = useSearchParams();
  const set = useCallback(
    (patch: Patch, mode: "push" | "replace" = "replace") => {
      const next = new URLSearchParams(window.location.search);
      for (const [key, value] of Object.entries(patch)) {
        if (value) next.set(key, value);
        else next.delete(key);
      }
      const query = next.toString();
      router[mode](query ? `${pathname}?${query}` : pathname, { scroll: false });
    },
    [pathname, router],
  );
  return [params, set] as const;
}

/** Opens the memory detail sheet (`?memory=<id>`) from any page. */
export function useOpenMemory(): (id: string | null) => void {
  const [, set] = useQueryParams();
  return useCallback((id) => set({ memory: id }, "push"), [set]);
}

/** A tab name kept in `?tab=`. Unknown or missing values fall back to the first tab. */
export function useTabParam<T extends string>(valid: readonly T[]): [T, (tab: T) => void] {
  const [params, set] = useQueryParams();
  const raw = params.get("tab");
  const tab = valid.find((name) => name === raw) ?? valid[0];
  return [tab, (next) => set({ tab: next === valid[0] ? null : next })];
}
