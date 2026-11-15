"use client";

import { FormEvent, useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { cn } from "cn";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { cairn } from "@/lib/cairn";
import { signInFailureMessage } from "@/lib/session";

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
  const [ready, setReady] = useState(false);

  // An empty server has no account to sign in to: offer first-account setup instead.
  useEffect(() => {
    let cancelled = false;
    cairn<{ bootstrap_required?: boolean }>("health")
      .then((health) => {
        if (!cancelled) {
          setBootstrap(health.bootstrap_required === true);
        }
      })
      .catch(() => {})
      .finally(() => {
        if (!cancelled) {
          setReady(true);
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setPending(true);
    setError(null);
    try {
      await cairn(bootstrap ? "auth/bootstrap" : "auth/login", {
        method: "POST",
        body: JSON.stringify({ email, password }),
      });
      setEmail("");
      setPassword("");
      router.push("/projects");
    } catch (cause) {
      const message =
        cause instanceof Error
          ? cause.message
          : bootstrap
            ? "Account setup failed"
            : "Sign-in failed";
      setError(signInFailureMessage(message));
    } finally {
      setPending(false);
    }
  }

  return (
    <div className={cn("flex flex-col gap-6", className)} {...props}>
      <Card className="overflow-hidden p-0 py-0">
        <CardContent className="grid p-0 md:grid-cols-2">
          <form
            onSubmit={onSubmit}
            className="p-6 md:p-8"
            aria-describedby={error ? "sign-in-error" : undefined}
          >
            <FieldGroup>
              <div className="flex flex-col gap-2">
                <h1 className="text-2xl font-semibold tracking-tight">
                  {bootstrap ? "Create the first account" : "Sign in"}
                </h1>
                <p className="text-sm text-muted-foreground">
                  {bootstrap
                    ? "This server has no account yet. The account you create here owns it. Use a password of at least 12 characters."
                    : "Use your Cairn account email and password."}
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
                />
              </Field>
              {error ? (
                <p id="sign-in-error" role="alert" className="text-sm text-destructive">
                  {error}
                </p>
              ) : null}
              <Field>
                <Button type="submit" disabled={pending || !ready}>
                  {bootstrap
                    ? pending
                      ? "Creating account…"
                      : "Create account"
                    : pending
                      ? "Signing in…"
                      : "Sign in"}
                </Button>
              </Field>
            </FieldGroup>
          </form>
          <div className="relative hidden min-h-80 flex-col justify-between bg-muted p-8 md:flex">
            <p className="text-sm font-medium">Cairn</p>
            <p className="max-w-xs text-sm text-muted-foreground">
              Project memory stays with the account that owns it.
            </p>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
