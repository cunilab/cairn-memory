import { useCallback, useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { cairn } from "@/lib/cairn";
import { LOGIN_PATH, SessionRequiredError } from "@/lib/session";
import type { AuditEvent } from "@/lib/generated/AuditEvent";

const PAGE = 50;

type State = { key: string; events: AuditEvent[]; full: boolean; error: string | null };

/**
 * One slice of the audit log, newest first: `actor` picks Cairn's own actions or the person's,
 * `types` narrows to some actions. Older pages pass the last row's `before` + `before_id`
 * (rows within one second would be lost with `before` alone).
 */
export function useAudit(actor: "system" | "person", types: readonly string[] = []) {
  const router = useRouter();
  const typeParam = types.join(",");
  const base = `auth/audit?limit=${PAGE}&actor=${actor}${typeParam ? `&type=${encodeURIComponent(typeParam)}` : ""}`;
  const [state, setState] = useState<State | null>(null);
  const [more, setMore] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const key = `${base}#${attempt}`;
  useResetOnHide(() => setState(null));

  const fail = useCallback(
    (cause: unknown, prefix: string) => {
      if (cause instanceof SessionRequiredError) {
        router.replace(LOGIN_PATH);
        return null;
      }
      return `${prefix} ${cause instanceof Error ? cause.message : "Check that the server is running, then retry."}`;
    },
    [router],
  );

  useEffect(() => {
    let cancelled = false;
    cairn<AuditEvent[]>(base)
      .then((events) => !cancelled && setState({ key, events, full: events.length === PAGE, error: null }))
      .catch((cause: unknown) => {
        const error = cancelled ? null : fail(cause, "Activity could not be loaded.");
        if (error) setState({ key, events: [], full: false, error });
      });
    return () => {
      cancelled = true;
    };
  }, [base, key, fail]);

  const current = state?.key === key ? state : null;

  const loadOlder = useCallback(async () => {
    const last = current?.events.at(-1);
    if (!current || !last) return;
    setMore(true);
    try {
      const older = await cairn<AuditEvent[]>(`${base}&before=${last.created_at}&before_id=${encodeURIComponent(last.audit_id)}`);
      setState((s) => (s && s.key === key ? { ...s, events: [...s.events, ...older], full: older.length === PAGE } : s));
    } catch (cause) {
      const error = fail(cause, "Older activity could not be loaded.");
      if (error) setState((s) => (s && s.key === key ? { ...s, error } : s));
    } finally {
      setMore(false);
    }
  }, [current, base, key, fail]);

  const retry = useCallback(() => {
    setState(null);
    setAttempt((n) => n + 1);
  }, []);

  return {
    events: current && !current.error ? current.events : null,
    error: current?.error ?? null,
    hasMore: current?.full ?? false,
    loadingMore: more,
    loadOlder,
    retry,
  };
}
