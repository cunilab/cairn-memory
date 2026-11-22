"use client";

import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import type { ColumnDef } from "@tanstack/react-table";
import {
  CheckIcon,
  CircleAlertIcon,
  CircleCheckIcon,
  CircleDashedIcon,
  CircleHelpIcon,
  EyeIcon,
  EyeOffIcon,
  KeyRoundIcon,
  MoreHorizontalIcon,
  PlusIcon,
} from "lucide-react";
import { toast } from "sonner";
import { DeviceStatusBadge } from "@/components/badges";
import { CopyButton } from "@/components/copy-button";
import { DataTable } from "@/components/data-table";
import { ErrorAlert, LoadingBlock } from "@/components/states";
import { Time } from "@/components/time";
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
import { Spinner } from "@/components/ui/spinner";
import { useQueryParams } from "@/hooks/use-query-params";
import { useResetOnHide } from "@/hooks/use-reset-on-hide";
import { revokeDeviceImpact, rotateDeviceImpact } from "@/lib/action-copy";
import { baselineOf, checkedInSince, type Baseline } from "@/lib/device-checkin";
import { cairn } from "@/lib/cairn";
import { LOGIN_PATH, SessionRequiredError } from "@/lib/session";
import type { DeviceView } from "@/lib/generated/DeviceView";
import type { DeviceRegistration } from "@/lib/generated/DeviceRegistration";
import type { Health } from "@/lib/generated/Health";

const AGENTS = [
  { value: "claude", label: "Claude" },
  { value: "codex", label: "Codex" },
  { value: "opencode", label: "OpenCode" },
] as const;
type Agent = (typeof AGENTS)[number]["value"];

const HEALTH = {
  working: { text: "Working", icon: CircleCheckIcon },
  update_needed: { text: "Update needed", icon: CircleAlertIcon },
  stale: { text: "Not seen recently", icon: CircleHelpIcon },
  never_connected: { text: "Never connected", icon: CircleDashedIcon },
} as const;

const POLL_MS = 3000;
const POLL_LIMIT_MS = 10 * 60 * 1000;

type DeviceAction = { device: DeviceView; kind: "revoke" | "rotate" };
type Issued = { registration: DeviceRegistration; baseline: Baseline; rotated: boolean };

/** Step 2 and 3: the token (shown once), the setup command, and the wait for the first check-in. */
function TokenStep({
  issued,
  agent,
  serverUrl,
  onAgent,
  onDone,
  connected,
  waitExpired,
}: {
  issued: Issued;
  agent: Agent;
  serverUrl: string | null;
  onAgent: (agent: Agent) => void;
  onDone: () => void;
  connected: string | null;
  waitExpired: boolean;
}) {
  const [shown, setShown] = useState(false);
  const token = issued.registration.token;
  const command = `cairn setup ${serverUrl ?? "<server-url>"} --agent ${agent}`;
  return (
    <div className="flex flex-col gap-4">
      <DialogHeader>
        <DialogTitle className="break-words">Token for {issued.registration.device.name}</DialogTitle>
        <DialogDescription>
          {issued.rotated
            ? "The old token stops working now. Run the setup command again on that machine."
            : "Copy the token now. It is shown once and cannot be retrieved later."}
        </DialogDescription>
      </DialogHeader>
      <ol className="flex flex-col gap-4 text-sm">
        <li className="flex flex-col gap-2">
          <span className="font-medium">1. Copy your token</span>
          <div className="flex flex-wrap items-center gap-2">
            <code className="min-w-0 flex-1 rounded-md border bg-muted p-3 font-mono text-sm break-all">
              {shown ? token : "•".repeat(24)}
            </code>
            <Button
              type="button"
              variant="outline"
              className="min-h-11 sm:min-h-8"
              aria-pressed={shown}
              aria-label={shown ? "Hide token" : "Show token"}
              onClick={() => setShown((value) => !value)}
            >
              {shown ? <EyeOffIcon /> : <EyeIcon />}
              {shown ? "Hide" : "Show"}
            </Button>
            <CopyButton text={token} label="Copy token" onCopied={() => toast.success("Token copied")} />
          </div>
        </li>
        <li className="flex flex-col gap-2">
          <span className="font-medium">2. Run on that machine</span>
          <fieldset className="flex flex-wrap gap-x-4 gap-y-1">
            <legend className="sr-only">Agent</legend>
            {AGENTS.map((item) => (
              <label key={item.value} className="flex min-h-8 items-center gap-2">
                <input
                  type="radio"
                  name="agent"
                  value={item.value}
                  checked={agent === item.value}
                  onChange={() => onAgent(item.value)}
                />
                {item.label}
              </label>
            ))}
          </fieldset>
          <div className="flex flex-wrap items-center gap-2">
            <code className="min-w-0 flex-1 rounded-md border bg-muted p-3 font-mono text-sm break-all">{command}</code>
            <CopyButton text={command} label="Copy" />
          </div>
          {serverUrl === null ? (
            <p className="text-muted-foreground">Replace <code>&lt;server-url&gt;</code> with this server&apos;s address.</p>
          ) : null}
          <p className="text-muted-foreground">Paste the token when it asks. It never goes in your shell history.</p>
        </li>
        <li className="flex flex-col gap-1">
          <span className="font-medium">3. Start a new {agent} session</span>
          <p aria-live="polite" className="flex items-center gap-2 text-muted-foreground">
            {connected !== null ? (
              <>
                <CheckIcon className="size-4 text-success" aria-hidden />
                Connected{connected ? `: ${connected}` : ""}. This device checked in.
              </>
            ) : waitExpired ? (
              "Still waiting. Run cairn doctor on that machine to see what is wrong."
            ) : (
              <>
                <Spinner aria-hidden />
                Waiting for the first check-in…
              </>
            )}
          </p>
        </li>
      </ol>
      <DialogFooter>
        <Button type="button" onClick={onDone}>
          Done
        </Button>
      </DialogFooter>
    </div>
  );
}

export function Devices() {
  const router = useRouter();
  const [params, set] = useQueryParams();
  const [devices, setDevices] = useState<DeviceView[] | null>(null);
  const [name, setName] = useState("");
  const [agent, setAgent] = useState<Agent>("claude");
  const [createOpen, setCreateOpen] = useState(false);
  const [issued, setIssued] = useState<Issued | null>(null);
  const [connected, setConnected] = useState<string | null>(null);
  const [waitExpired, setWaitExpired] = useState(false);
  const [action, setAction] = useState<DeviceAction | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [serverUrl, setServerUrl] = useState<string | null>(null);
  const acting = useRef(false);

  useResetOnHide(() => {
    acting.current = false;
    setDevices(null);
    setName("");
    setCreateOpen(false);
    setIssued(null);
    setConnected(null);
    setWaitExpired(false);
    setAction(null);
    setError(null);
    setActionError(null);
    setPending(false);
  });

  const requireSignIn = useCallback(() => {
    setDevices(null);
    setIssued(null);
    router.replace(LOGIN_PATH);
  }, [router]);

  const load = useCallback(() => cairn<DeviceView[]>("devices"), []);

  // The setup command needs the address `cairn setup` is given.
  useEffect(() => {
    let cancelled = false;
    cairn<Health>("health")
      .then((health) => !cancelled && setServerUrl(health.public_url))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    load()
      .then((listed) => {
        if (!cancelled) {
          setDevices(listed);
          setError(null);
        }
      })
      .catch((cause: unknown) => {
        if (cancelled) return;
        if (cause instanceof SessionRequiredError) {
          requireSignIn();
          return;
        }
        setError(`Devices could not be loaded. ${cause instanceof Error ? cause.message : "Check that the server is running, then retry."}`);
        setDevices([]);
      });
    return () => {
      cancelled = true;
    };
  }, [load, requireSignIn, attempt]);

  // The palette's "Create device token" lands here with ?create=1.
  const wantsCreate = params.get("create") === "1";
  useEffect(() => {
    if (wantsCreate) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setCreateOpen(true);
      set({ create: null });
    }
  }, [wantsCreate, set]);

  // Poll for the first check-in while the token dialog is open and the tab is visible.
  const issuedId = issued?.registration.device.device_id;
  const baseline = issued?.baseline;
  useEffect(() => {
    if (!issuedId) return;
    const started = Date.now();
    const timer = setInterval(() => {
      if (document.visibilityState !== "visible") return;
      if (Date.now() - started > POLL_LIMIT_MS) {
        setWaitExpired(true);
        clearInterval(timer);
        return;
      }
      load()
        .then((listed) => {
          setDevices(listed);
          const row = listed.find((d) => d.device_id === issuedId);
          if (row && baseline && checkedInSince(row, baseline)) {
            setConnected([row.agent, row.client_version].filter(Boolean).join(" "));
            clearInterval(timer);
          }
        })
        .catch(() => {});
    }, POLL_MS);
    return () => clearInterval(timer);
  }, [issuedId, baseline, load]);

  function issue(registration: DeviceRegistration, rotated: boolean) {
    setConnected(null);
    setWaitExpired(false);
    setIssued({ registration, baseline: baselineOf(registration.device), rotated });
  }

  async function createDevice(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (acting.current || name.trim() === "") return;
    acting.current = true;
    setPending(true);
    setActionError(null);
    try {
      const registration = await cairn<DeviceRegistration>("devices/register", {
        method: "POST",
        body: JSON.stringify({ name: name.trim() }),
      });
      issue(registration, false);
      setName("");
      toast.success("Device token created");
    } catch (cause) {
      if (cause instanceof SessionRequiredError) requireSignIn();
      else setActionError(cause instanceof Error ? cause.message : "Request failed");
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
      setError("The device token was created, but the list could not be reloaded.");
    } finally {
      acting.current = false;
      setPending(false);
    }
  }

  async function confirmAction() {
    if (!action || acting.current) return;
    const { device: target, kind } = action;
    acting.current = true;
    setPending(true);
    setActionError(null);
    try {
      if (kind === "rotate") {
        issue(
          await cairn<DeviceRegistration>("devices/rotate", {
            method: "POST",
            body: JSON.stringify({ deviceId: target.device_id }),
          }),
          true,
        );
        setAction(null);
        toast.success("Token rotated");
      } else {
        await cairn("devices/revoke", { method: "POST", body: JSON.stringify({ deviceId: target.device_id }) });
        toast.success("Device revoked");
      }
    } catch (cause) {
      if (cause instanceof SessionRequiredError) requireSignIn();
      else setActionError(cause instanceof Error ? cause.message : "Request failed");
      acting.current = false;
      setPending(false);
      return;
    }
    try {
      setDevices(await load());
      setAction(null);
    } catch (cause) {
      if (cause instanceof SessionRequiredError) {
        requireSignIn();
        return;
      }
      setAction(null);
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

  const columns = useMemo<ColumnDef<DeviceView>[]>(() => {
    const cols: ColumnDef<DeviceView>[] = [
      {
        accessorKey: "name",
        header: "Device",
        cell: ({ row }) => {
          const d = row.original;
          return (
            <div className="flex flex-col gap-0.5">
              <span className="font-medium break-words">{d.name}</span>
              {/* The Agent, Client and Last check-in columns are hidden below md; phones read them here. */}
              <dl className="flex flex-col gap-0.5 text-xs text-muted-foreground md:hidden">
                <div className="flex gap-1">
                  <dt>Agent:</dt>
                  <dd>{d.agent ?? "—"}</dd>
                </div>
                <div className="flex gap-1">
                  <dt>Client:</dt>
                  <dd>{d.client_version ?? "—"}</dd>
                </div>
                <div className="flex gap-1">
                  <dt>Last check-in:</dt>
                  <dd>{d.last_hook_at === null ? "Never" : <Time value={d.last_hook_at} className="whitespace-nowrap" />}</dd>
                </div>
              </dl>
            </div>
          );
        },
      },
    ];
    cols.push(
      {
        id: "agent",
        header: "Agent",
        accessorFn: (d) => d.agent ?? "",
        cell: ({ row }) => row.original.agent ?? <span className="text-muted-foreground">—</span>,
        meta: { className: "hidden md:table-cell" },
      },
      {
        id: "client",
        header: "Client",
        accessorFn: (d) => d.client_version ?? "",
        cell: ({ row }) => row.original.client_version ?? <span className="text-muted-foreground">—</span>,
        meta: { className: "hidden md:table-cell" },
      },
      {
        id: "last_hook_at",
        header: "Last check-in",
        accessorFn: (d) => d.last_hook_at ?? 0,
        cell: ({ row }) =>
          row.original.last_hook_at === null ? (
            <span className="text-xs text-muted-foreground">Never</span>
          ) : (
            <Time value={row.original.last_hook_at} className="text-xs whitespace-nowrap text-muted-foreground" />
          ),
        meta: { className: "hidden md:table-cell" },
      },
      {
        accessorKey: "status",
        header: "Status",
        cell: ({ row }) => {
          const d = row.original;
          const entry = d.status !== "revoked" && d.health ? HEALTH[d.health] : null;
          if (!entry) return <DeviceStatusBadge status={d.status} />;
          const Icon = entry.icon;
          return (
            <span className="inline-flex flex-col gap-0.5">
              <span className="inline-flex items-center gap-1.5 text-sm">
                <Icon className="size-4" aria-hidden />
                {entry.text}
              </span>
              {d.health_reason ? <span className="text-xs text-muted-foreground">{d.health_reason}</span> : null}
              {d.fix_command && d.health !== "working" ? (
                <span className="inline-flex flex-wrap items-center gap-1">
                  <code className="font-mono text-xs break-all">{d.fix_command}</code>
                  <CopyButton text={d.fix_command} label="Copy fix command" />
                </span>
              ) : null}
            </span>
          );
        },
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
                      className="size-11 sm:size-7"
                      disabled={pending}
                      aria-label={`Actions for ${row.original.name}`}
                    />
                  }
                >
                  <MoreHorizontalIcon />
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="w-44">
                  <DropdownMenuItem
                    onClick={() => {
                      setActionError(null);
                      setAction({ device: row.original, kind: "rotate" });
                    }}
                  >
                    Rotate token
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem
                    variant="destructive"
                    onClick={() => {
                      setActionError(null);
                      setAction({ device: row.original, kind: "revoke" });
                    }}
                  >
                    Revoke device
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          ),
        meta: { className: "w-12" },
      },
    );
    return cols;
  }, [pending]);

  const createButton = (
    <Button
      type="button"
      onClick={() => {
        setActionError(null);
        setCreateOpen(true);
      }}
    >
      <PlusIcon />
      Create device token
    </Button>
  );

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="max-w-prose text-sm text-muted-foreground">
          A device token lets an agent read and write memory for this account. Only you can create or revoke one.
        </p>
        {createButton}
      </div>
      {error ? <ErrorAlert message={error} onRetry={() => { setError(null); setDevices(null); setAttempt((n) => n + 1); }} /> : null}
      {devices === null ? <LoadingBlock label="Loading devices…" rows={4} /> : null}
      {devices !== null && (!error || devices.length > 0) ? (
        <DataTable
          label="devices"
          data={devices}
          columns={columns}
          getRowId={(device) => device.device_id}
          filterPlaceholder="Filter devices"
          initialSort={[{ id: "last_hook_at", desc: true }]}
          empty={
            <Empty className="border-0 p-6">
              <EmptyHeader>
                <EmptyMedia variant="icon">
                  <KeyRoundIcon />
                </EmptyMedia>
                <EmptyTitle>No devices yet</EmptyTitle>
                <EmptyDescription>Create a device token, then run the setup command on the machine where your agent runs.</EmptyDescription>
              </EmptyHeader>
              <EmptyContent>{createButton}</EmptyContent>
            </Empty>
          }
        />
      ) : null}

      <Dialog
        open={createOpen || issued !== null}
        onOpenChange={(open) => {
          if (open || pending) return;
          setCreateOpen(false);
          setIssued(null);
          setActionError(null);
        }}
      >
        <DialogContent className="max-h-[90svh] overflow-y-auto sm:max-w-xl">
          {issued ? (
            <TokenStep
              key={issued.registration.device.device_id + issued.registration.token}
              issued={issued}
              agent={agent}
              serverUrl={serverUrl}
              onAgent={setAgent}
              connected={connected}
              waitExpired={waitExpired}
              onDone={() => {
                setCreateOpen(false);
                setIssued(null);
              }}
            />
          ) : (
            <form className="flex flex-col gap-4" onSubmit={(event) => void createDevice(event)}>
              <DialogHeader>
                <DialogTitle>Create device token</DialogTitle>
                <DialogDescription>
                  Name the machine or agent that will use the token, for example a laptop or a CI runner.
                </DialogDescription>
              </DialogHeader>
              <FieldGroup>
                <Field>
                  <FieldLabel htmlFor="device-name">Name</FieldLabel>
                  <Input
                    id="device-name"
                    value={name}
                    onChange={(event) => setName(event.target.value)}
                    autoComplete="off"
                    required
                  />
                </Field>
              </FieldGroup>
              {actionError ? (
                <Alert variant="destructive">
                  <AlertDescription className="text-destructive">{actionError}</AlertDescription>
                </Alert>
              ) : null}
              <DialogFooter>
                <Button type="submit" disabled={pending}>
                  {pending ? <Spinner aria-hidden /> : null}
                  {pending ? "Creating…" : "Create device token"}
                </Button>
              </DialogFooter>
            </form>
          )}
        </DialogContent>
      </Dialog>

      <AlertDialog
        open={action !== null}
        onOpenChange={(open) => {
          if (pending || open) return;
          setAction(null);
          setActionError(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{action?.kind === "rotate" ? "Rotate this token?" : "Revoke this device?"}</AlertDialogTitle>
            <AlertDialogDescription className="text-left [overflow-wrap:anywhere]">
              {action
                ? action.kind === "rotate"
                  ? rotateDeviceImpact(action.device.name)
                  : revokeDeviceImpact(action.device.name)
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
            <AlertDialogAction variant="destructive" disabled={pending} onClick={() => void confirmAction()}>
              {pending ? "Working…" : action?.kind === "rotate" ? "Rotate token" : "Revoke device"}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
