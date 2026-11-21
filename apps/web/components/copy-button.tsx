"use client";

import { useState } from "react";
import { CheckIcon, CopyIcon } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";

/** Copies `text`. The label flips to "Copied" and the toast is polite, so screen readers hear it. */
export function CopyButton({
  text,
  label,
  onCopied,
  className,
}: {
  text: string;
  label: string;
  onCopied?: () => void;
  className?: string;
}) {
  const [copied, setCopied] = useState(false);
  return (
    <Button
      type="button"
      variant="outline"
      className={className ?? "min-h-11 sm:min-h-8"}
      onClick={() => {
        navigator.clipboard.writeText(text).then(
          () => {
            setCopied(true);
            onCopied?.();
          },
          () => toast.error("Could not copy. Select the text and copy it by hand."),
        );
      }}
    >
      {copied ? <CheckIcon /> : <CopyIcon />}
      {copied ? "Copied" : label}
    </Button>
  );
}
