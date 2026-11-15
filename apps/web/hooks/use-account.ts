import { useEffect, useState } from "react";
import { cairn } from "@/lib/cairn";
import type { Account } from "@/lib/types";

/** The signed-in account: undefined while loading, null when it could not be read. */
export function useAccount(): Account | null | undefined {
  const [account, setAccount] = useState<Account | null | undefined>(undefined);
  useEffect(() => {
    let cancelled = false;
    cairn<Account>("auth/me")
      .then((me) => {
        if (!cancelled) {
          setAccount(me);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setAccount(null);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);
  return account;
}
