import type { InstructionSource } from "@/lib/generated/InstructionSource";

/** The longest text the server stores per field, in bytes (the server checks it too). */
export const MAX_INSTRUCTION_BYTES = 600;

export const SOURCE_LABEL: Record<InstructionSource, string> = {
  project: "this project's override",
  global: "the global text",
  default: "the built-in default",
};

/** UTF-8 bytes of the text as the server counts them (it trims first). */
export function byteLength(text: string): number {
  return new TextEncoder().encode(text.trim()).length;
}

/** "123 / 600 bytes", or how far over the limit the text is. */
export function byteCounter(text: string, max: number = MAX_INSTRUCTION_BYTES): string {
  const used = byteLength(text);
  return used > max ? `${used} / ${max} bytes (${used - max} over)` : `${used} / ${max} bytes`;
}

export function overLimit(text: string, max: number = MAX_INSTRUCTION_BYTES): boolean {
  return byteLength(text) > max;
}

/** What the hook appends to the session brief, exactly: the brief, a newline, the text. */
export function sampleStart(text: string): string {
  return `Cairn session brief …\n${text.trim()}`;
}

/** What the agent is told at Stop: `{files}` becomes the number of edited files. */
export function sampleStop(text: string, files: number = 3): string {
  return text.trim().replaceAll("{files}", String(files));
}

/** A change to send: only the fields whose text differs from what is saved. Empty clears. */
export function changedFields<Key extends string>(
  draft: Record<Key, string>,
  saved: Record<Key, string | null>,
): Partial<Record<Key, string>> {
  const out: Partial<Record<Key, string>> = {};
  for (const key of Object.keys(draft) as Key[]) {
    if (draft[key].trim() !== (saved[key] ?? "")) out[key] = draft[key].trim();
  }
  return out;
}
