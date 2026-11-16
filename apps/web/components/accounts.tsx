"use client";

import { FormEvent, useState } from "react";
import { useRouter } from "next/navigation";
import { ShieldAlertIcon } from "lucide-react";
import { toast } from "sonner";
import { ConsoleShell } from "@/components/console-shell";
import { PageHeader } from "@/components/page-header";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { Spinner } from "@/components/ui/spinner";
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
  const [pending, setPending] = useState(false);

  useResetOnHide(() => {
    setEmail("");
    setPassword("");
    setConfirm("");
    setError(null);
    setPending(false);
  });

  async function createAccount(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
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
      toast.success(`Account created for ${email.trim()}.`);
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

  return (
    <ConsoleShell crumbs={[{ label: "Admin" }, { label: "Accounts" }]}>
      <PageHeader
        title="Accounts"
        description="Create an account for a teammate. They sign in with this email and password. Your own session is not changed."
      />
      {account === undefined ? (
        <div role="status" className="max-w-md">
          <span className="sr-only">Loading…</span>
          <Skeleton className="h-80 w-full" />
        </div>
      ) : account?.is_admin ? (
        <Card className="w-full max-w-md">
          <CardHeader>
            <CardTitle>
              <h2>New account</h2>
            </CardTitle>
            <CardDescription>Use a password of at least {MIN_PASSWORD} characters.</CardDescription>
          </CardHeader>
          <CardContent>
            <form className="flex flex-col gap-4" onSubmit={(event) => void createAccount(event)}>
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
                  <FieldDescription>Type it again so a typo does not lock them out.</FieldDescription>
                </Field>
              </FieldGroup>
              {error ? (
                <Alert variant="destructive">
                  <AlertDescription className="text-destructive">{error}</AlertDescription>
                </Alert>
              ) : null}
              <div>
                <Button type="submit" disabled={pending}>
                  {pending ? <Spinner aria-hidden /> : null}
                  {pending ? "Creating account…" : "Create account"}
                </Button>
              </div>
            </form>
          </CardContent>
        </Card>
      ) : (
        <Empty className="border">
          <EmptyHeader>
            <EmptyMedia variant="icon">
              <ShieldAlertIcon />
            </EmptyMedia>
            <EmptyTitle>Admins only</EmptyTitle>
            <EmptyDescription role="alert">Only an admin can create accounts.</EmptyDescription>
          </EmptyHeader>
        </Empty>
      )}
    </ConsoleShell>
  );
}
