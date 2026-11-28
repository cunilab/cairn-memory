"use client";

import { FormEvent, useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { cn } from "cn";
import { CairnLogo, CairnMark } from "@/components/cairn-logo";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { ApiError, cairn } from "@/lib/cairn";
import type { Health } from "@/lib/generated/Health";
import { safeNext, signInFailureMessage } from "@/lib/session";

export function LoginForm({
  className,
  ...props
}: React.ComponentProps<"div">) {
  const router = useRouter();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [bootstrap, setBootstrap] = useState(false);
  const [connection, setConnection] = useState<"checking" | "ready" | "down">("checking");
  const [probe, setProbe] = useState(0);

  // An empty server has no account to sign in to: offer first-account setup instead.
  useEffect(() => {
    let cancelled = false;
    cairn<Health>("health")
      .then((health) => {
        if (cancelled) return;
        setBootstrap(health.bootstrap_required);
        setConnection("ready");
      })
      .catch(() => {
        if (!cancelled) setConnection("down");
      });
    return () => {
      cancelled = true;
    };
  }, [probe]);

  async function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (connection !== "ready") return;
    setPending(true);
    setError(null);
    try {
      await cairn(bootstrap ? "auth/bootstrap" : "auth/login", {
        method: "POST",
        body: JSON.stringify({ email, password }),
      });
      setEmail("");
      setPassword("");
      const next = safeNext(new URLSearchParams(window.location.search).get("next"));
      router.push(next ?? "/dashboard");
    } catch (cause) {
      const message =
        cause instanceof Error
          ? cause.message
          : bootstrap
            ? "Account setup failed"
            : "Sign-in failed";
      setError(signInFailureMessage(cause instanceof ApiError ? cause.code : null, message));
    } finally {
      setPending(false);
    }
  }

  return (
    <div
      className={cn(
        "grid overflow-hidden rounded-xl border bg-card shadow-sm md:grid-cols-[minmax(0,1fr)_minmax(0,0.9fr)]",
        className,
      )}
      {...props}
    >
      <form
        onSubmit={onSubmit}
        className="flex flex-col gap-6 p-6 md:p-10"
        aria-describedby={error ? "sign-in-error" : undefined}
      >
        <CairnLogo className="md:hidden" />
        <FieldGroup>
          <div className="flex flex-col gap-1.5">
            <h1 className="text-2xl font-semibold">
              {connection === "checking"
                ? "Checking server"
                : bootstrap
                  ? "Create the first account"
                  : "Sign in"}
            </h1>
            <p className="text-sm text-muted-foreground">
              {connection === "down"
                ? "Can't reach the server."
                : connection === "checking"
                  ? "Checking the server connection."
                  : bootstrap
                    ? "This server has no account yet. The account you create here owns it."
                    : "Server connection ready."}
            </p>
          </div>
          <Field>
            <FieldLabel htmlFor="email">Email</FieldLabel>
            <Input
              id="email"
              name="email"
              type="email"
              autoComplete="username"
              value={email}
              onChange={(event) => setEmail(event.target.value)}
              required
              disabled={connection !== "ready"}
            />
          </Field>
          <Field>
            <FieldLabel htmlFor="password">Password</FieldLabel>
            <Input
              id="password"
              name="password"
              type="password"
              autoComplete={bootstrap ? "new-password" : "current-password"}
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              required
              disabled={connection !== "ready"}
            />
          </Field>
          {error ? (
            <Alert id="sign-in-error" variant="destructive">
              <AlertDescription className="text-destructive">{error}</AlertDescription>
            </Alert>
          ) : null}
          <Field>
            {connection === "down" ? (
              <Button type="button" size="lg" onClick={() => { setConnection("checking"); setProbe((n) => n + 1); }}>
                Retry
              </Button>
            ) : (
              <Button type="submit" size="lg" disabled={pending || connection !== "ready"}>
                {pending || connection === "checking" ? <Spinner aria-hidden /> : null}
                {connection === "checking"
                  ? "Checking server"
                  : bootstrap
                    ? pending
                      ? "Creating account…"
                      : "Create account"
                    : pending
                      ? "Signing in…"
                      : "Sign in"}
              </Button>
            )}
          </Field>
        </FieldGroup>
      </form>
      <div className="relative hidden min-h-96 flex-col justify-between overflow-hidden border-l bg-sidebar p-10 md:flex">
        <CairnLogo />
        <CairnMark
          className="pointer-events-none absolute -right-10 -bottom-10 size-72 opacity-[0.14] [transform:rotate(8deg)]"
        />
        <p className="relative max-w-xs text-lg leading-snug font-medium text-balance">
          Project memory stays with the account that owns it.
        </p>
      </div>
    </div>
  );
}
