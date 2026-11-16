import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

function label(value: string): string {
  const text = value.replace(/_/g, " ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

export function RoleBadge({ role }: { role: string }) {
  return <Badge variant={role === "owner" ? "brand" : "outline"}>{label(role)}</Badge>;
}

export function ScopeBadge({ scope }: { scope: string | null | undefined }) {
  if (!scope) {
    return <span className="text-muted-foreground">—</span>;
  }
  return <Badge variant={scope === "project" ? "secondary" : "outline"}>{label(scope)}</Badge>;
}

export function KindBadge({ kind }: { kind: string | null | undefined }) {
  if (!kind) {
    return <span className="text-muted-foreground">—</span>;
  }
  return <Badge variant="outline">{label(kind)}</Badge>;
}

export function SensitivityBadge({ sensitivity }: { sensitivity: string | null | undefined }) {
  if (!sensitivity) {
    return <span className="text-muted-foreground">—</span>;
  }
  const risky = sensitivity === "sensitive" || sensitivity === "secret";
  return (
    <Badge variant={risky ? "destructive" : sensitivity === "private" ? "secondary" : "outline"}>
      {label(sensitivity)}
    </Badge>
  );
}

export function DeviceStatusBadge({ status }: { status: string }) {
  return status === "revoked" ? (
    <Badge variant="destructive">Revoked</Badge>
  ) : (
    <Badge variant="success">Active</Badge>
  );
}

/** Freshness is a 0-100 score; the number is always text, the bar is decoration. */
export function Freshness({ value, className }: { value: number | null | undefined; className?: string }) {
  if (value === null || value === undefined) {
    return <span className="text-muted-foreground">—</span>;
  }
  return (
    <span className={cn("inline-flex items-center gap-2 tabular-nums", className)}>
      <span aria-hidden className="h-1 w-8 rounded-full bg-muted">
        <span
          className="block h-1 rounded-full bg-primary"
          style={{ width: `${Math.max(0, Math.min(100, value))}%` }}
        />
      </span>
      {value}
    </span>
  );
}

/** Identifiers stay mono and wrap; the full value is always in the title and the accessible text. */
export function IdText({ id, className }: { id: string; className?: string }) {
  return <code className={cn("font-mono text-xs break-all text-muted-foreground", className)}>{id}</code>;
}
