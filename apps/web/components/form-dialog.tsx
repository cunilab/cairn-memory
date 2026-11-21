"use client";

import { useState, type FormEvent, type ReactNode } from "react";
import { useRouter } from "next/navigation";
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
import { FieldGroup } from "@/components/ui/field";
import { Spinner } from "@/components/ui/spinner";
import { LOGIN_PATH, SessionRequiredError } from "@/lib/session";

export const MIN_PASSWORD = 12;

/** Rejects a new password that is too short or typed differently twice. */
export function checkNewPassword(password: string, confirm: string) {
  if (password.length < MIN_PASSWORD) {
    throw new Error(`Use a password of at least ${MIN_PASSWORD} characters.`);
  }
  if (password !== confirm) {
    throw new Error("The passwords do not match.");
  }
}

type FormDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: ReactNode;
  submitLabel: string;
  /** Receives the named inputs. Throw to show the message in the dialog; return to close it. */
  onSubmit: (data: FormData) => Promise<void>;
  children: ReactNode;
};

function Body({
  title,
  description,
  submitLabel,
  onSubmit,
  onOpenChange,
  children,
}: Omit<FormDialogProps, "open">) {
  const router = useRouter();
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    setPending(true);
    setError(null);
    try {
      await onSubmit(data);
      onOpenChange(false);
    } catch (cause) {
      if (cause instanceof SessionRequiredError) {
        router.replace(LOGIN_PATH);
        return;
      }
      setError(cause instanceof Error ? cause.message : "Request failed");
      setPending(false);
    }
  }

  return (
    <form className="contents" onSubmit={(event) => void submit(event)}>
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{description}</DialogDescription>
      </DialogHeader>
      <FieldGroup>{children}</FieldGroup>
      {error ? (
        <Alert variant="destructive">
          <AlertDescription className="text-destructive">{error}</AlertDescription>
        </Alert>
      ) : null}
      <DialogFooter>
        <Button type="button" variant="outline" disabled={pending} onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        <Button type="submit" disabled={pending}>
          {pending ? <Spinner aria-hidden /> : null}
          {submitLabel}
        </Button>
      </DialogFooter>
    </form>
  );
}

/**
 * A dialog around one form. Inputs are uncontrolled and the body unmounts on close, so every
 * open starts empty and a half-typed password is never kept.
 */
export function FormDialog({ open, onOpenChange, ...body }: FormDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <Body {...body} onOpenChange={onOpenChange} />
      </DialogContent>
    </Dialog>
  );
}
