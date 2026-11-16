import { useSyncExternalStore } from "react";
import { readTheme, setTheme, subscribeTheme, type Theme } from "@/lib/theme";

/** The saved theme (System until the person picks one) and a setter that applies it at once. */
export function useTheme(): [Theme, (theme: Theme) => void] {
  const theme = useSyncExternalStore(subscribeTheme, readTheme, () => "system" as const);
  return [theme, setTheme];
}
