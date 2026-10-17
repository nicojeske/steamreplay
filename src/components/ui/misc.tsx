import type { ReactNode } from "react";
import { CircleAlert, Info, LoaderCircle, TriangleAlert } from "lucide-react";

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="kbd">{children}</kbd>;
}

export function Badge({ tone = "neutral", children }: { tone?: "neutral" | "accent" | "clip" | "warn"; children: ReactNode }) {
  return <span className={`badge badge-${tone}`}>{children}</span>;
}

export function Spinner({ size = 16 }: { size?: number }) {
  return <LoaderCircle className="spinner" size={size} aria-hidden />;
}

/** 0–1, or `null` for an unknown amount. */
export function ProgressBar({ value, label }: { value: number | null; label?: string }) {
  return (
    <div
      className="progress"
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={value === null ? undefined : Math.round(value * 100)}
    >
      <div className={value === null ? "indeterminate" : undefined} style={value === null ? undefined : { width: `${value * 100}%` }} />
    </div>
  );
}

export function Callout({ tone = "info", children }: { tone?: "info" | "warn" | "danger"; children: ReactNode }) {
  const Icon = tone === "info" ? Info : tone === "warn" ? TriangleAlert : CircleAlert;
  return (
    <div className={`callout callout-${tone}`}>
      <Icon size={16} aria-hidden />
      <div>{children}</div>
    </div>
  );
}

type EmptyProps = { icon?: ReactNode; title: ReactNode; children?: ReactNode; className?: string };

export function EmptyState({ icon, title, children, className }: EmptyProps) {
  return (
    <div className={`empty-state ${className ?? ""}`}>
      {icon && <div className="empty-icon">{icon}</div>}
      <h2>{title}</h2>
      {children}
    </div>
  );
}
