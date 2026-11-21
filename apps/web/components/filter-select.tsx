"use client";

import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

const ALL = "__all__";

export type FilterOption = { value: string; label: string };

/** A labelled select whose "all" choice is the empty string, so it maps straight to a URL param. */
export function FilterSelect({
  label,
  value,
  onChange,
  options,
  allLabel,
  disabled,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  options: FilterOption[];
  allLabel: string;
  disabled?: boolean;
}) {
  const items = [{ value: ALL, label: allLabel }, ...options];
  return (
    <label className="grid min-w-0 gap-1 text-xs font-medium text-muted-foreground">
      {label}
      <Select
        items={items}
        value={value || ALL}
        disabled={disabled}
        onValueChange={(next) => onChange(next === ALL || next === null ? "" : String(next))}
      >
        <SelectTrigger className="w-full" aria-label={label}>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {items.map((item) => (
            <SelectItem key={item.value} value={item.value}>
              {item.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </label>
  );
}
