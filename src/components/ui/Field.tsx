import type { ReactNode } from "react";

type Props = { label: ReactNode; description?: ReactNode; children?: ReactNode; stacked?: boolean };

/** A settings row: label and description on the left, the control on the right (or below when `stacked`). */
export function Field({ label, description, children, stacked }: Props) {
  return (
    <div className={`field ${stacked ? "field-stacked" : ""}`}>
      <div className="field-text">
        <div className="field-label">{label}</div>
        {description && <div className="field-description">{description}</div>}
      </div>
      {children && <div className="field-control">{children}</div>}
    </div>
  );
}
