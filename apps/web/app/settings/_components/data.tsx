"use client";

import { DownloadIcon, UploadIcon } from "lucide-react";
import { Button } from "@/components/ui/button";

/**
 * TODO(7a): Export (`GET /memories/export`) and Import (`POST /memories/import?dry_run=1`) do
 * not exist on the server yet. The tab shows the final layout with both actions off, so
 * nothing here calls an endpoint that is missing.
 */
export function DataTab() {
  return (
    <div className="flex max-w-xl flex-col gap-6">
      <section className="flex flex-col gap-2">
        <h2 className="text-sm font-semibold">Export</h2>
        <p className="text-sm text-muted-foreground">Download all memories you own as JSON.</p>
        <div>
          <Button type="button" variant="outline" disabled>
            <DownloadIcon />
            Export
          </Button>
        </div>
      </section>
      <section className="flex flex-col gap-2">
        <h2 className="text-sm font-semibold">Import</h2>
        <p className="text-sm text-muted-foreground">
          Choose a Cairn export file. You see a summary of what would be added before anything is written.
        </p>
        <div>
          <Button type="button" variant="outline" disabled>
            <UploadIcon />
            Choose file
          </Button>
        </div>
      </section>
      <p className="text-sm text-muted-foreground">Export and import are not available on this server yet.</p>
    </div>
  );
}
