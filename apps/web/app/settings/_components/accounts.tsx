"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { useRouter } from "next/navigation";
import type { ColumnDef } from "@tanstack/react-table";
import { MoreHorizontalIcon, PlusIcon, ShieldAlertIcon, UsersIcon } from "lucide-react";
import { toast } from "sonner";
import { DataTable } from "@/components/data-table";
import { checkNewPassword, FormDialog, MIN_PASSWORD } from "@/components/form-dialog";
import { ErrorAlert } from "@/components/states";
import { Avatar, AvatarFallback } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { useAccount } from "@/hooks/use-account";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { cairn } from "@/lib/cairn";
import { formatTime } from "@/lib/report";
import { LOGIN_PATH, SessionRequiredError } from "@/lib/session";
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
  const [transferring, setTransferring] = useState<AccountRow | null>(null);
  const [resetting, setResetting] = useState<AccountRow | null>(null);
  const admin = account?.is_admin === true;

  useResetOnHide(() => {
    setRows(null);
    setError(null);
    setCreating(false);
    setResetting(null);
    setTransferring(null);
  });

  const load = useCallback(async () => {
    try {
      setRows(await cairn<AccountRow[]>("auth/accounts"));
      setError(null);
    } catch (cause) {
      if (cause instanceof SessionRequiredError) {
        router.replace(LOGIN_PATH);
        return;
      }
      setError(`Accounts could not be loaded. ${cause instanceof Error ? cause.message : "Check that the server is running, then retry."}`);
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
                <DropdownMenuItem onClick={() => setTransferring(row.original)}>
                  Transfer project ownership
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
    <div className="flex flex-col gap-4">
      <p className="max-w-prose text-sm text-muted-foreground">
        Everyone who can sign in to this Cairn server. Register accounts for teammates and reset a password when someone is locked out.
      </p>
      {account === undefined ? (
        <div role="status">
          <span className="sr-only">Loading…</span>
          <Skeleton className="h-64 w-full" />
        </div>
      ) : admin ? (
        <>
          {error ? <ErrorAlert message={error} onRetry={() => void load()} /> : null}
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
                  Register account
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
        title="Register account"
        description="They sign in with this email and password. Your own session is not changed."
        submitLabel="Register account"
        onSubmit={async (data) => {
          const email = String(data.get("email")).trim();
          const password = String(data.get("password"));
          checkNewPassword(password, String(data.get("confirm")));
          await cairn("auth/register", {
            method: "POST",
            body: JSON.stringify({ email, password }),
          });
          toast.success(`Account registered for ${email}`);
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
            Set a new password for{" "}
            <span className="font-medium break-all text-foreground">{resetting?.email}</span>. This
            revokes their web sessions, so they must sign in again with the new password.
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
          toast.success(
            `Password reset for ${resetting.email}. They are signed out of web sessions.`,
          );
        }}
      >
        <PasswordFields label="New password" prefix="reset" />
      </FormDialog>
      <FormDialog
        open={transferring !== null}
        onOpenChange={(open) => !open && setTransferring(null)}
        title="Transfer project ownership"
        description={`Make ${transferring?.email ?? "this account"} the owner of a project they already belong to. This records an audit event.`}
        submitLabel="Transfer ownership"
        onSubmit={async (data) => {
          if (!transferring) return;
          const projectId = String(data.get("projectId")).trim();
          await cairn(`projects/${encodeURIComponent(projectId)}/ownership`, {
            method: "POST",
            body: JSON.stringify({ personId: transferring.person_id }),
          });
          toast.success("Project ownership transferred.");
        }}
      >
        <Field>
          <FieldLabel htmlFor="transfer-project-id">Project ID</FieldLabel>
          <Input id="transfer-project-id" name="projectId" required autoComplete="off" />
        </Field>
      </FormDialog>
    </div>
  );
}
