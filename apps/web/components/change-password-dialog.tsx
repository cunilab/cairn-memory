"use client";

import { toast } from "sonner";
import { checkNewPassword, FormDialog, MIN_PASSWORD } from "@/components/form-dialog";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { cairn } from "@/lib/cairn";
import { SessionRequiredError } from "@/lib/session";

/** The signed-in person changes their own password. A wrong current password stays in the dialog. */
export function ChangePasswordDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <FormDialog
      open={open}
      onOpenChange={onOpenChange}
      title="Change password"
      description="Other sessions of this account stay signed in until you sign them out."
      submitLabel="Change password"
      onSubmit={async (data) => {
        const next = String(data.get("password"));
        checkNewPassword(next, String(data.get("confirm")));
        try {
          await cairn("auth/password", {
            method: "POST",
            body: JSON.stringify({
              current_password: String(data.get("current")),
              new_password: next,
            }),
          });
        } catch (cause) {
          // The server answers a wrong current password with 401; it is not an expired session.
          if (
            cause instanceof SessionRequiredError ||
            (cause instanceof Error && cause.message === "invalid account credentials")
          ) {
            throw new Error("The current password is incorrect.");
          }
          throw cause;
        }
        toast.success("Password changed.");
      }}
    >
      <Field>
        <FieldLabel htmlFor="current-password">Current password</FieldLabel>
        <Input id="current-password" name="current" type="password" autoComplete="current-password" required />
      </Field>
      <Field>
        <FieldLabel htmlFor="profile-password">New password</FieldLabel>
        <Input id="profile-password" name="password" type="password" autoComplete="new-password" required />
        <FieldDescription>At least {MIN_PASSWORD} characters.</FieldDescription>
      </Field>
      <Field>
        <FieldLabel htmlFor="profile-confirm">Confirm new password</FieldLabel>
        <Input id="profile-confirm" name="confirm" type="password" autoComplete="new-password" required />
      </Field>
    </FormDialog>
  );
}
