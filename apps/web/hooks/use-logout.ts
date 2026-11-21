import { useCallback } from "react";
import { useRouter } from "next/navigation";
import { toast } from "sonner";
import { cairn } from "@/lib/cairn";
import { LOGIN_PATH, SessionRequiredError } from "@/lib/session";

/** Ends the web session and returns to sign-in. A session that is already gone counts as done. */
export function useLogout(): () => Promise<void> {
  const router = useRouter();
  return useCallback(async () => {
    try {
      await cairn("auth/logout", { method: "POST" });
    } catch (cause) {
      if (!(cause instanceof SessionRequiredError)) {
        const message = cause instanceof Error ? cause.message : "Logout failed";
        toast.error(`Could not log out. ${message}`);
        return;
      }
    }
    router.replace(LOGIN_PATH);
  }, [router]);
}
