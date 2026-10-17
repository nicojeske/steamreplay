import type { ReactNode } from "react";

export type Toast = { id: number; text: string };

/** Error toasts (click to dismiss), below any persistent `children` such as the update prompt. */
export function Toasts({ toasts, onDismiss, children }: { toasts: Toast[]; onDismiss: (id: number) => void; children?: ReactNode }) {
  return (
    <div className="toasts">
      {children}
      {toasts.map((t) => (
        <div key={t.id} className="toast" role="alert" onClick={() => onDismiss(t.id)}>
          {t.text}
        </div>
      ))}
    </div>
  );
}
