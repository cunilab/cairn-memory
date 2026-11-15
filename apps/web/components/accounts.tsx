"use client";

import { FormEvent, useState } from "react";
import { useRouter } from "next/navigation";
import { ConsoleShell } from "@/components/console-shell";
import { Button } from "@/components/ui/button";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { useAccount } from "@/hooks/use-account";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { cairn } from "@/lib/cairn";
import { SessionRequiredError } from "@/lib/session";

const MIN_PASSWORD = 12;

export function Accounts() {
  const router = useRouter();
  const account = useAccount();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  useResetOnHide(() => {
    setEmail("");
    setPassword("");
    setConfirm("");
    setError(null);
    setNotice(null);
    setPending(false);
  });

  async function createAccount(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setNotice(null);
    if (password.length < MIN_PASSWORD) {
      setError(`Use a password of at least ${MIN_PASSWORD} characters.`);
      return;
    }
    if (password !== confirm) {
      setError("The passwords do not match.");
      return;
    }
    setPending(true);
    setError(null);
    try {
      await cairn("auth/register", {
        method: "POST",
        body: JSON.stringify({ email: email.trim(), password }),
      });
      setNotice(`Account created for ${email.trim()}.`);
      setEmail("");
      setPassword("");
      setConfirm("");
    } catch (cause) {
      if (cause instanceof SessionRequiredError) {
        router.replace("/");
        return;
      }
      setError(cause instanceof Error ? cause.message : "Request failed");
    } finally {
      setPending(false);
    }
  }

  async function logout() {
    try {
      await cairn("auth/logout", { method: "POST" });
    } catch (cause) {
      if (!(cause instanceof SessionRequiredError)) {
        setError(cause instanceof Error ? `Could not log out. ${cause.message}` : "Logout failed");
        return;
      }
    }
    router.replace("/");
  }

  return (
    <ConsoleShell
      title="Accounts"
      action={
        <Button type="button" variant="outline" onClick={() => void logout()}>
          Log out
        </Button>
      }
    >
      <div className="flex min-w-0 flex-col gap-6">
        {account === undefined ? (
          <p role="status" className="text-sm text-muted-foreground">
            Loading…
          </p>
        ) : account?.is_admin ? (
          <>
            <p className="max-w-prose text-sm text-muted-foreground">
              Create an account for a teammate. They sign in with this email and password. Your
              own session is not changed.
            </p>
            <form className="flex max-w-sm flex-col gap-3" onSubmit={(event) => void createAccount(event)}>
              <FieldGroup>
                <Field>
                  <FieldLabel htmlFor="account-email">Email</FieldLabel>
                  <Input
                    id="account-email"
                    type="email"
                    value={email}
                    onChange={(event) => setEmail(event.target.value)}
                    autoComplete="off"
                    required
                  />
                </Field>
                <Field>
                  <FieldLabel htmlFor="account-password">Password</FieldLabel>
                  <Input
                    id="account-password"
                    type="password"
                    value={password}
                    onChange={(event) => setPassword(event.target.value)}
                    autoComplete="new-password"
                    required
                  />
                </Field>
                <Field>
                  <FieldLabel htmlFor="account-confirm">Confirm password</FieldLabel>
                  <Input
                    id="account-confirm"
                    type="password"
                    value={confirm}
                    onChange={(event) => setConfirm(event.target.value)}
                    autoComplete="new-password"
                    required
                  />
                </Field>
              </FieldGroup>
              {error ? (
                <p role="alert" className="text-sm text-destructive">
                  {error}
                </p>
              ) : null}
              {notice ? (
                <p role="status" className="text-sm text-muted-foreground">
                  {notice}
                </p>
              ) : null}
              <div>
                <Button type="submit" disabled={pending}>
                  {pending ? "Creating account…" : "Create account"}
                </Button>
              </div>
            </form>
          </>
        ) : (
          <p role="alert" className="text-sm text-destructive">
            Only an admin can create accounts.
          </p>
        )}
      </div>
    </ConsoleShell>
  );
}
