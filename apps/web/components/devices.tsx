"use client";

import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import type { ColumnDef } from "@tanstack/react-table";
import { CheckIcon, CopyIcon, KeyRoundIcon, MoreHorizontalIcon, PlusIcon } from "lucide-react";
import { toast } from "sonner";
import { DeviceStatusBadge, IdText } from "@/components/badges";
import { ConsoleShell } from "@/components/console-shell";
import { DataTable } from "@/components/data-table";
import { PageHeader } from "@/components/page-header";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { Spinner } from "@/components/ui/spinner";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { revokeDeviceImpact, rotateDeviceImpact } from "@/lib/action-copy";
import { cairn } from "@/lib/cairn";
import { formatTime } from "@/lib/report";
import { SessionRequiredError } from "@/lib/session";
import type { Device, DeviceRegistration } from "@/lib/types";

/** Registration and last use share a stamp until the device is first used. */
function lastUsed(device: Device): string {
  return device.last_seen_at === device.registered_at
    ? "Never"
    : formatTime(device.last_seen_at);
}

type DeviceAction = { device: Device; kind: "revoke" | "rotate" };

export function Devices() {
  const router = useRouter();
  const [devices, setDevices] = useState<Device[]>([]);
  const [name, setName] = useState("");
  const [createOpen, setCreateOpen] = useState(false);
  const [created, setCreated] = useState<DeviceRegistration | null>(null);
  const [copied, setCopied] = useState(false);
  const [revoking, setRevoking] = useState<DeviceAction | null>(null);
  const [dialogDevice, setDialogDevice] = useState<DeviceAction | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [pending, setPending] = useState(false);
  const [sessionRequired, setSessionRequired] = useState(false);
  const acting = useRef(false);

  useResetOnHide(() => {
    acting.current = false;
    setDevices([]);
    setName("");
    setCreateOpen(false);
    setCreated(null);
    setCopied(false);
    setRevoking(null);
    setDialogDevice(null);
    setError(null);
    setActionError(null);
    setLoading(true);
    setPending(false);
    setSessionRequired(false);
  });

  const requireSignIn = useCallback(() => {
    setSessionRequired(true);
    setDevices([]);
    setCreated(null);
    router.replace("/");
  }, [router]);

  const load = useCallback(() => cairn<Device[]>("devices"), []);

  useEffect(() => {
    let cancelled = false;
    load()
      .then((listed) => {
        if (!cancelled) {
          setDevices(listed);
        }
      })
      .catch((cause: unknown) => {
        if (cancelled) {
          return;
        }
        if (cause instanceof SessionRequiredError) {
          requireSignIn();
          return;
        }
        const message = cause instanceof Error ? cause.message : "Could not load devices";
        setError(`Devices could not be loaded. ${message}`);
      })
      .finally(() => {
        if (!cancelled) {
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [load, requireSignIn]);

  async function createDevice(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (acting.current || name.trim() === "") {
      return;
    }
    acting.current = true;
    setPending(true);
    setActionError(null);
    setCopied(false);
    try {
      const registration = await cairn<DeviceRegistration>("devices/register", {
        method: "POST",
        body: JSON.stringify({ name: name.trim() }),
      });
      setCreated(registration);
      setName("");
      setCreateOpen(false);
      toast.success("Device created.");
    } catch (cause) {
      if (cause instanceof SessionRequiredError) {
        requireSignIn();
      } else {
        setActionError(cause instanceof Error ? cause.message : "Request failed");
      }
      acting.current = false;
      setPending(false);
      return;
    }
    try {
      setDevices(await load());
    } catch (cause) {
      if (cause instanceof SessionRequiredError) {
        requireSignIn();
        return;
      }
      setError("The device was created, but the list could not be reloaded.");
    } finally {
      acting.current = false;
      setPending(false);
    }
  }

  async function copyToken() {
    if (!created) {
      return;
    }
    try {
      await navigator.clipboard.writeText(created.token);
      setCopied(true);
      toast.success("Token copied.");
    } catch {
      toast.error("Could not copy. Select the token and copy it by hand.");
    }
  }

  async function confirmDevice() {
    if (!revoking || acting.current) {
      return;
    }
    const { device: target, kind } = revoking;
    acting.current = true;
    setPending(true);
    setActionError(null);
    try {
      if (kind === "rotate") {
        const registration = await cairn<DeviceRegistration>("devices/rotate", {
          method: "POST",
          body: JSON.stringify({ deviceId: target.device_id }),
        });
        setCreated(registration);
        setCopied(false);
        toast.success("Token rotated.");
      } else {
        await cairn("devices/revoke", {
          method: "POST",
          body: JSON.stringify({ deviceId: target.device_id }),
        });
        toast.success("Device revoked.");
      }
    } catch (cause) {
      if (cause instanceof SessionRequiredError) {
        requireSignIn();
      } else {
        setActionError(cause instanceof Error ? cause.message : "Request failed");
      }
      acting.current = false;
      setPending(false);
      return;
    }
    try {
      setDevices(await load());
      setRevoking(null);
    } catch (cause) {
      if (cause instanceof SessionRequiredError) {
        requireSignIn();
        return;
      }
      setRevoking(null);
      setError(
        kind === "rotate"
          ? "The token was rotated, but the list could not be reloaded."
          : "The device was revoked, but the list could not be reloaded.",
      );
    } finally {
      acting.current = false;
      setPending(false);
    }
  }

  function ask(device: Device, kind: DeviceAction["kind"]) {
    setActionError(null);
    setDialogDevice({ device, kind });
    setRevoking({ device, kind });
  }

  const columns = useMemo<ColumnDef<Device>[]>(
    () => [
      {
        accessorKey: "name",
        header: "Device",
        cell: ({ row }) => (
          <div className="flex min-w-0 flex-col gap-0.5">
            <span className="font-medium break-words">{row.original.name}</span>
            <IdText id={row.original.device_id} />
          </div>
        ),
      },
      {
        accessorKey: "status",
        header: "Status",
        cell: ({ row }) => <DeviceStatusBadge status={row.original.status} />,
      },
      {
        accessorKey: "registered_at",
        header: "Created",
        sortingFn: (a, b) => Number(a.original.registered_at) - Number(b.original.registered_at),
        cell: ({ row }) => (
          <span className="text-xs whitespace-nowrap text-muted-foreground">
            {formatTime(row.original.registered_at)}
          </span>
        ),
        meta: { className: "hidden md:table-cell" },
      },
      {
        accessorKey: "last_seen_at",
        header: "Last used",
        sortingFn: (a, b) => Number(a.original.last_seen_at) - Number(b.original.last_seen_at),
        cell: ({ row }) => (
          <span className="text-xs whitespace-nowrap text-muted-foreground">
            {lastUsed(row.original)}
          </span>
        ),
        meta: { className: "hidden sm:table-cell" },
      },
      {
        id: "actions",
        header: () => <span className="sr-only">Actions</span>,
        enableSorting: false,
        cell: ({ row }) =>
          row.original.status === "revoked" ? null : (
            <div className="flex justify-end">
              <DropdownMenu>
                <DropdownMenuTrigger
                  render={
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-sm"
                      disabled={pending}
                      aria-label={`Actions for ${row.original.name}`}
                    />
                  }
                >
                  <MoreHorizontalIcon />
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="w-44">
                  <DropdownMenuItem onClick={() => ask(row.original, "rotate")}>
                    Rotate token
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem variant="destructive" onClick={() => ask(row.original, "revoke")}>
                    Revoke device
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          ),
        meta: { className: "w-10" },
      },
    ],
    [pending],
  );

  if (sessionRequired) {
    return (
      <main className="mx-auto flex min-h-svh w-full max-w-sm flex-1 items-center px-6">
        <p className="text-sm text-muted-foreground">Sign in is required.</p>
      </main>
    );
  }

  const createButton = (
    <Button
      type="button"
      onClick={() => {
        setActionError(null);
        setCreateOpen(true);
      }}
    >
      <PlusIcon />
      Create device
    </Button>
  );

  return (
    <ConsoleShell crumbs={[{ label: "Devices" }]}>
      <PageHeader
        title="Devices"
        description="A device token lets an agent read and write memory for this account. Only you can create or revoke one, here."
        actions={createButton}
      />
      {error ? (
        <Alert variant="destructive">
          <AlertDescription className="text-destructive">{error}</AlertDescription>
        </Alert>
      ) : null}

      {created ? (
        <Card className="min-w-0 border-primary/40 ring-primary/30" role="status">
          <CardHeader>
            <CardTitle className="break-words">Token for {created.device.name}</CardTitle>
            <CardDescription>
              Copy it now. It is shown once and cannot be retrieved later.
            </CardDescription>
          </CardHeader>
          <CardContent className="flex min-w-0 flex-col gap-3">
            <code className="rounded-md border bg-muted p-3 font-mono text-sm break-all select-all">
              {created.token}
            </code>
            <div className="flex flex-wrap items-center gap-2">
              <Button type="button" onClick={() => void copyToken()}>
                {copied ? <CheckIcon /> : <CopyIcon />}
                {copied ? "Copied" : "Copy token"}
              </Button>
              <Button
                type="button"
                variant="outline"
                onClick={() => {
                  setCreated(null);
                  setCopied(false);
                }}
              >
                I have copied it
              </Button>
            </div>
          </CardContent>
        </Card>
      ) : null}

      {loading ? (
        <div role="status" className="flex flex-col gap-2">
          <span className="sr-only">Loading devices…</span>
          <Skeleton className="h-8 w-64" />
          <Skeleton className="h-40 w-full" />
        </div>
      ) : null}
      {!loading && (!error || devices.length > 0) ? (
        <DataTable
          label="devices"
          data={devices}
          columns={columns}
          getRowId={(device) => device.device_id}
          filterPlaceholder="Filter devices"
          initialSort={[{ id: "registered_at", desc: true }]}
          empty={
            <Empty className="border-0 p-6">
              <EmptyHeader>
                <EmptyMedia variant="icon">
                  <KeyRoundIcon />
                </EmptyMedia>
                <EmptyTitle>No devices</EmptyTitle>
                <EmptyDescription>Create a device to give an agent access.</EmptyDescription>
              </EmptyHeader>
              <EmptyContent>{createButton}</EmptyContent>
            </Empty>
          }
        />
      ) : null}
      <Dialog
        open={createOpen}
        onOpenChange={(open) => {
          if (pending && !open) {
            return;
          }
          setCreateOpen(open);
          if (!open) {
            setActionError(null);
          }
        }}
      >
        <DialogContent>
          <form className="flex flex-col gap-4" onSubmit={(event) => void createDevice(event)}>
            <DialogHeader>
              <DialogTitle>Create a device</DialogTitle>
              <DialogDescription>
                Name the machine or agent that will use the token, for example a laptop or a CI
                runner. The token is shown once.
              </DialogDescription>
            </DialogHeader>
            <FieldGroup>
              <Field>
                <FieldLabel htmlFor="device-name">Device name</FieldLabel>
                <Input
                  id="device-name"
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                  autoComplete="off"
                  required
                />
              </Field>
            </FieldGroup>
            {actionError && !revoking ? (
              <Alert variant="destructive">
                <AlertDescription className="text-destructive">{actionError}</AlertDescription>
              </Alert>
            ) : null}
            <DialogFooter>
              <Button type="submit" disabled={pending || name.trim() === ""}>
                {pending && !revoking ? <Spinner aria-hidden /> : null}
                {pending && !revoking ? "Creating…" : "Create device"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <AlertDialog
        open={revoking !== null}
        onOpenChange={(open) => {
          if (pending || open) {
            return;
          }
          setRevoking(null);
          setActionError(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {dialogDevice?.kind === "rotate" ? "Rotate this token?" : "Revoke this device?"}
            </AlertDialogTitle>
            <AlertDialogDescription className="text-left [overflow-wrap:anywhere]">
              {dialogDevice
                ? dialogDevice.kind === "rotate"
                  ? rotateDeviceImpact(dialogDevice.device.name)
                  : revokeDeviceImpact(dialogDevice.device.name)
                : ""}
            </AlertDialogDescription>
          </AlertDialogHeader>
          {actionError ? (
            <Alert variant="destructive">
              <AlertDescription className="text-destructive">{actionError}</AlertDescription>
            </Alert>
          ) : null}
          <AlertDialogFooter>
            <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={pending}
              onClick={() => void confirmDevice()}
            >
              {pending
                ? "Working…"
                : dialogDevice?.kind === "rotate"
                  ? "Rotate token"
                  : "Revoke device"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </ConsoleShell>
  );
}
