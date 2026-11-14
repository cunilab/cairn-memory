import { useEffectEvent, useLayoutEffect } from "react";

// Cache Components keeps a hidden route mounted (Activity). Clear identity-scoped state when
// the route is hidden, before paint, so the next identity never sees the previous one's data
// and no in-flight request from the old identity can repaint it after revival.
export function useResetOnHide(reset: () => void) {
  const onHide = useEffectEvent(reset);
  useLayoutEffect(() => () => onHide(), []);
}
