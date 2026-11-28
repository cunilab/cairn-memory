/** Title and summary sizes: agent cards show the suggested size; the server refuses more than the maximum. */
export const TEXT_SIZES = {
  title: { suggested: 80, max: 200 },
  summary: { suggested: 240, max: 1000 },
} as const;

/** The counter beside a field: a soft note past the suggested size, never an error below the maximum. */
export function textSizeHint(field: keyof typeof TEXT_SIZES, text: string): { label: string; long: boolean } {
  const { suggested, max } = TEXT_SIZES[field];
  const count = Array.from(text).length;
  const long = count > suggested;
  return { label: long ? `${count}/${suggested} suggested, ${max} max` : `${count}/${suggested}`, long };
}
