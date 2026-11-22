import { useCallback, useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { cairn } from "@/lib/cairn";
import { LOGIN_PATH, SessionRequiredError } from "@/lib/session";
import type { AuditEvent } from "@/lib/generated/AuditEvent";

type State = { events: AuditEvent[] | null; error: string | null };

/**
 * The person's audit events, newest first.
 * TODO(7a): `GET /auth/audit` becomes paged (`before`) and filterable (`actor`, `type`); the
 * timeline and audit tab then fetch their own slices instead of splitting one list.
 */
export function useAudit() {
  const router = useRouter();
  const [state, setState] = useState<State>({ events: null, error: null });
  const [attempt, setAttempt] = useState(0);
  useResetOnHide(() => setState({ events: null, error: null }));
  useEffect(() => {
    let cancelled = false;
    cairn<AuditEvent[]>("auth/audit")
      .then((events) => !cancelled && setState({ events, error: null }))
      .catch((cause: unknown) => {
        if (cancelled) return;
        if (cause instanceof SessionRequiredError) {
          router.replace(LOGIN_PATH);
          return;
        }
        setState({
          events: null,
          error: `Activity could not be loaded. ${cause instanceof Error ? cause.message : "Check that the server is running, then retry."}`,
        });
      });
    return () => {
      cancelled = true;
    };
  }, [attempt, router]);
  const retry = useCallback(() => {
    setState({ events: null, error: null });
    setAttempt((n) => n + 1);
  }, []);
  return { ...state, retry };
}
