import { cairn } from "@/lib/cairn";

/** Downloads the person's own memories as `cairn-export-YYYY-MM-DD.json`. */
export async function downloadExport(): Promise<void> {
  const body = await cairn<unknown>("memories/export");
  const url = URL.createObjectURL(new Blob([JSON.stringify(body, null, 2)], { type: "application/json" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = `cairn-export-${new Date().toISOString().slice(0, 10)}.json`;
  link.click();
  URL.revokeObjectURL(url);
}
