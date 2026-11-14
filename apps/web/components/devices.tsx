"use client";

import { FormEvent, useCallback, useEffect, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import { ConsoleShell } from "@/components/console-shell";
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
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { revokeDeviceImpact } from "@/lib/action-copy";
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

export function Devices() {
  const router = useRouter();
  const [devices, setDevices] = useState<Device[]>([]);
  const [name, setName] = useState("");
  const [created, setCreated] = useState<DeviceRegistration | null>(null);
  const [copied, setCopied] = useState(false);
  const [revoking, setRevoking] = useState<Device | null>(null);
  const [dialogDevice, setDialogDevice] = useState<Device | null>(null);
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
    } catch {
      setActionError("Could not copy. Select the token and copy it by hand.");
    }
  }

  async function revokeDevice() {
    if (!revoking || acting.current) {
      return;
    }
    const target = revoking;
    acting.current = true;
    setPending(true);
    setActionError(null);
    try {
      await cairn("devices/revoke", {
        method: "POST",
        body: JSON.stringify({ deviceId: target.device_id }),
      });
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
      setError("The device was revoked, but the list could not be reloaded.");
    } finally {
      acting.current = false;
      setPending(false);
    }
  }

  async function logout() {
    try {
      await cairn("auth/logout", { method: "POST" });
    } catch (cause) {
      if (cause instanceof SessionRequiredError) {
        requireSignIn();
        return;
      }
      const message = cause instanceof Error ? cause.message : "Logout failed";
      setError(`Could not log out. ${message}`);
      return;
    }
    router.replace("/");
  }

  if (sessionRequired) {
    return (
      <main className="mx-auto flex min-h-svh w-full max-w-sm flex-1 items-center px-6">
        <p className="text-sm text-muted-foreground">Sign in is required.</p>
      </main>
    );
  }

  return (
    <ConsoleShell
      title="Devices"
      action={
        <Button type="button" variant="outline" onClick={() => void logout()}>
          Log out
        </Button>
      }
    >
      <div className="flex min-w-0 flex-col gap-6">
        <p className="max-w-prose text-sm text-muted-foreground">
          A device token lets an agent read and write memory for this account. Only you can
          create or revoke one, here.
        </p>
        {error ? (
          <p role="alert" className="text-sm text-destructive">
            {error}
          </p>
        ) : null}

        {created ? (
          <Card className="min-w-0" role="status">
            <CardHeader>
              <CardTitle className="break-words">Token for {created.device.name}</CardTitle>
              <CardDescription>
                Copy it now. It is shown once and cannot be retrieved later.
              </CardDescription>
            </CardHeader>
            <CardContent className="flex min-w-0 flex-col gap-3">
              <code className="rounded-md border bg-muted p-3 text-sm break-all select-all">
                {created.token}
              </code>
              <div className="flex flex-wrap items-center gap-2">
                <Button type="button" onClick={() => void copyToken()}>
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

        <section className="flex min-w-0 flex-col gap-3">
          <h2 className="text-base font-medium">Create a device</h2>
          <form className="flex max-w-sm flex-col gap-3" onSubmit={(event) => void createDevice(event)}>
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
              <p role="alert" className="text-sm text-destructive">
                {actionError}
              </p>
            ) : null}
            <div>
              <Button type="submit" disabled={pending || name.trim() === ""}>
                {pending && !revoking ? "Creating…" : "Create device"}
              </Button>
            </div>
          </form>
        </section>

        <section className="flex min-w-0 flex-col gap-3">
          <h2 className="text-base font-medium">Your devices</h2>
          {loading ? (
            <p role="status" className="text-sm text-muted-foreground">
              Loading devices…
            </p>
          ) : null}
          {!loading && devices.length === 0 && !error ? (
            <div className="flex w-full min-w-0 flex-col items-center justify-center gap-2 rounded-xl border border-dashed p-6 text-center">
              <p className="text-sm font-medium">No devices</p>
              <p className="max-w-sm text-sm text-balance text-muted-foreground">
                Create a device to give an agent access.
              </p>
            </div>
          ) : null}
          <ul className="flex flex-col gap-3">
            {devices.map((device) => (
              <li key={device.device_id} className="min-w-0">
                <Card className="min-w-0">
                  <CardHeader>
                    <CardTitle className="break-words">{device.name}</CardTitle>
                    <CardDescription className="break-all">{device.device_id}</CardDescription>
                  </CardHeader>
                  <CardContent className="flex min-w-0 flex-col gap-3">
                    <dl className="grid min-w-0 grid-cols-[max-content_minmax(0,1fr)] gap-x-3 gap-y-1 text-sm">
                      <dt className="text-muted-foreground">Created</dt>
                      <dd>{formatTime(device.registered_at)}</dd>
                      <dt className="text-muted-foreground">Last used</dt>
                      <dd>{lastUsed(device)}</dd>
                      <dt className="text-muted-foreground">Status</dt>
                      <dd>{device.status === "revoked" ? "Revoked" : "Active"}</dd>
                    </dl>
                    {device.status === "revoked" ? null : (
                      <div>
                        <Button
                          type="button"
                          variant="destructive"
                          disabled={pending}
                          onClick={() => {
                            setActionError(null);
                            setDialogDevice(device);
                            setRevoking(device);
                          }}
                        >
                          Revoke device
                        </Button>
                      </div>
                    )}
                  </CardContent>
                </Card>
              </li>
            ))}
          </ul>
        </section>

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
              <AlertDialogTitle>Revoke this device?</AlertDialogTitle>
              <AlertDialogDescription className="text-left [overflow-wrap:anywhere]">
                {dialogDevice ? revokeDeviceImpact(dialogDevice.name) : ""}
              </AlertDialogDescription>
            </AlertDialogHeader>
            {actionError ? (
              <p role="alert" className="text-sm text-destructive">
                {actionError}
              </p>
            ) : null}
            <AlertDialogFooter>
              <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
              <AlertDialogAction
                variant="destructive"
                disabled={pending}
                onClick={() => void revokeDevice()}
              >
                {pending ? "Working…" : "Revoke device"}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </div>
    </ConsoleShell>
  );
}
