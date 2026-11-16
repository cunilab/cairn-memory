"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { useRouter } from "next/navigation";
import type { ColumnDef } from "@tanstack/react-table";
import { MoreHorizontalIcon, PlusIcon, ShieldAlertIcon, UsersIcon } from "lucide-react";
import { toast } from "sonner";
import { ConsoleShell } from "@/components/console-shell";
import { DataTable } from "@/components/data-table";
import { checkNewPassword, FormDialog, MIN_PASSWORD } from "@/components/form-dialog";
import { PageHeader } from "@/components/page-header";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { useAccount } from "@/hooks/use-account";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { cairn } from "@/lib/cairn";
import { formatTime } from "@/lib/report";
import { SessionRequiredError } from "@/lib/session";
import type { AccountRow } from "@/lib/types";

function PasswordFields({ label = "Password", prefix }: { label?: string; prefix: string }) {
  return (
    <>
      <Field>
        <FieldLabel htmlFor={`${prefix}-password`}>{label}</FieldLabel>
        <Input
          id={`${prefix}-password`}
          name="password"
          type="password"
          autoComplete="new-password"
          required
        />
        <FieldDescription>At least {MIN_PASSWORD} characters.</FieldDescription>
      </Field>
      <Field>
        <FieldLabel htmlFor={`${prefix}-confirm`}>Confirm password</FieldLabel>
        <Input
          id={`${prefix}-confirm`}
          name="confirm"
          type="password"
          autoComplete="new-password"
          required
        />
      </Field>
    </>
  );
}

export function Accounts() {
  const router = useRouter();
  const account = useAccount();
  const [rows, setRows] = useState<AccountRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [resetting, setResetting] = useState<AccountRow | null>(null);
  const admin = account?.is_admin === true;

  useResetOnHide(() => {
    setRows(null);
    setError(null);
    setCreating(false);
    setResetting(null);
  });

  const load = useCallback(async () => {
    try {
      setRows(await cairn<AccountRow[]>("auth/accounts"));
      setError(null);
    } catch (cause) {
      if (cause instanceof SessionRequiredError) {
        router.replace("/");
        return;
      }
      setError(cause instanceof Error ? cause.message : "Could not load accounts");
    }
  }, [router]);

  useEffect(() => {
    if (admin) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      void load();
    }
  }, [admin, load]);

  const columns = useMemo<ColumnDef<AccountRow>[]>(
    () => [
      {
        id: "email",
        header: "Email",
        accessorFn: (row) => row.email,
        cell: ({ row }) => (
          <div className="flex min-w-0 items-center gap-2.5">
            <Avatar className="size-6 rounded-md">
              <AvatarFallback className="rounded-md text-xs">
                {row.original.email.slice(0, 1).toUpperCase()}
              </AvatarFallback>
            </Avatar>
            <span className="text-sm break-all">{row.original.email}</span>
          </div>
        ),
      },
      {
        id: "role",
        header: "Role",
        accessorFn: (row) => (row.is_admin ? "Admin" : "User"),
        cell: ({ row }) =>
          row.original.is_admin ? (
            <Badge variant="brand">Admin</Badge>
          ) : (
            <Badge variant="outline">User</Badge>
          ),
      },
      {
        id: "created",
        header: "Created",
        accessorFn: (row) => Number(row.created_at),
        cell: ({ row }) => (
          <span className="text-xs whitespace-nowrap text-muted-foreground">
            {formatTime(row.original.created_at)}
          </span>
        ),
        meta: { className: "hidden sm:table-cell" },
      },
      {
        id: "actions",
        header: () => <span className="sr-only">Actions</span>,
        enableSorting: false,
        cell: ({ row }) => (
          <div className="flex justify-end">
            <DropdownMenu>
              <DropdownMenuTrigger
                render={
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    aria-label={`Actions for ${row.original.email}`}
                  />
                }
              >
                <MoreHorizontalIcon />
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" className="w-44">
                <DropdownMenuItem onClick={() => setResetting(row.original)}>
                  Reset password
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        ),
        meta: { className: "w-10" },
      },
    ],
    [],
  );

  return (
    <ConsoleShell crumbs={[{ label: "Admin" }, { label: "Accounts" }]}>
      <PageHeader
        title="Accounts"
        description="Everyone who can sign in to this Cairn server. Create accounts for teammates and reset a password when someone is locked out."
      />
      {account === undefined ? (
        <div role="status">
          <span className="sr-only">Loading…</span>
          <Skeleton className="h-64 w-full" />
        </div>
      ) : admin ? (
        <>
          {error ? (
            <Alert variant="destructive">
              <AlertDescription className="text-destructive">{error}</AlertDescription>
            </Alert>
          ) : null}
          {rows === null && !error ? (
            <div role="status">
              <span className="sr-only">Loading accounts…</span>
              <Skeleton className="h-64 w-full" />
            </div>
          ) : null}
          {rows ? (
            <DataTable
              label="accounts"
              data={rows}
              columns={columns}
              getRowId={(row) => row.person_id}
              filterPlaceholder="Filter accounts"
              initialSort={[{ id: "created", desc: true }]}
              toolbar={
                <Button type="button" className="sm:ml-auto" onClick={() => setCreating(true)}>
                  <PlusIcon />
                  New user
                </Button>
              }
              empty={
                <Empty className="border-0 p-6">
                  <EmptyHeader>
                    <EmptyMedia variant="icon">
                      <UsersIcon />
                    </EmptyMedia>
                    <EmptyTitle>No accounts</EmptyTitle>
                  </EmptyHeader>
                </Empty>
              }
            />
          ) : null}
        </>
      ) : (
        <Empty className="border">
          <EmptyHeader>
            <EmptyMedia variant="icon">
              <ShieldAlertIcon />
            </EmptyMedia>
            <EmptyTitle>Admins only</EmptyTitle>
            <EmptyDescription role="alert">Only an admin can manage accounts.</EmptyDescription>
          </EmptyHeader>
        </Empty>
      )}

      <FormDialog
        open={creating}
        onOpenChange={setCreating}
        title="New user"
        description="They sign in with this email and password. Your own session is not changed."
        submitLabel="Create user"
        onSubmit={async (data) => {
          const email = String(data.get("email")).trim();
          const password = String(data.get("password"));
          checkNewPassword(password, String(data.get("confirm")));
          await cairn("auth/register", { method: "POST", body: JSON.stringify({ email, password }) });
          toast.success(`Account created for ${email}.`);
          await load();
        }}
      >
        <Field>
          <FieldLabel htmlFor="new-user-email">Email</FieldLabel>
          <Input id="new-user-email" name="email" type="email" autoComplete="off" required />
        </Field>
        <PasswordFields prefix="new-user" />
      </FormDialog>

      <FormDialog
        open={resetting !== null}
        onOpenChange={(open) => !open && setResetting(null)}
        title="Reset password"
        description={
          <>
            Set a new password for <span className="font-medium break-all text-foreground">{resetting?.email}</span>.
            This signs them out everywhere, so they must sign in again with the new password.
          </>
        }
        submitLabel="Reset password"
        onSubmit={async (data) => {
          if (!resetting) {
            return;
          }
          const password = String(data.get("password"));
          checkNewPassword(password, String(data.get("confirm")));
          await cairn(`auth/accounts/${encodeURIComponent(resetting.person_id)}/password`, {
            method: "POST",
            body: JSON.stringify({ password }),
          });
          toast.success(`Password reset for ${resetting.email}. They are signed out everywhere.`);
        }}
      >
        <PasswordFields label="New password" prefix="reset" />
      </FormDialog>
    </ConsoleShell>
  );
}
