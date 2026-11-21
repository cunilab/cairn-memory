import Link from "next/link";

export type Bar = { label: string; value: number; href?: string };

/** Labelled horizontal bars. The number is always text, so the bar is decoration. */
export function BarList({ bars }: { bars: Bar[] }) {
  const max = Math.max(1, ...bars.map((bar) => bar.value));
  return (
    <ul className="flex min-w-0 flex-col gap-3">
      {bars.map((bar) => (
        <li key={bar.href ?? bar.label} className="flex min-w-0 flex-col gap-1">
          <div className="flex items-baseline justify-between gap-3 text-sm">
            {bar.href ? (
              <Link
                href={bar.href}
                className="min-w-0 break-words rounded-sm underline-offset-4 outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
              >
                {bar.label}
              </Link>
            ) : (
              <span className="min-w-0 break-words">{bar.label}</span>
            )}
            <span className="tabular-nums text-muted-foreground">{bar.value}</span>
          </div>
          <div aria-hidden className="h-2 rounded-full bg-muted">
            <div
              className="h-2 rounded-full bg-primary"
              style={{ width: `${(bar.value / max) * 100}%` }}
            />
          </div>
        </li>
      ))}
    </ul>
  );
}
