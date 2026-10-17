import type { ReactNode } from "react";
import { ToggleGroup } from "radix-ui";

type Option<T extends string> = { value: T; label: ReactNode; title?: string };

type Props<T extends string> = {
  value: T;
  options: Option<T>[];
  onChange: (value: T) => void;
  label: string;
  size?: "sm" | "md";
  disabled?: boolean;
};

/** One of a few options, as a row of joined buttons. */
export function Segmented<T extends string>({ value, options, onChange, label, size = "md", disabled }: Props<T>) {
  return (
    <ToggleGroup.Root
      type="single"
      className={`segmented segmented-${size}`}
      value={value}
      aria-label={label}
      disabled={disabled}
      onValueChange={(v) => v && onChange(v as T)}
    >
      {options.map((o) => (
        <ToggleGroup.Item key={o.value} value={o.value} className="segmented-item" title={o.title}>
          {o.label}
        </ToggleGroup.Item>
      ))}
    </ToggleGroup.Root>
  );
}
