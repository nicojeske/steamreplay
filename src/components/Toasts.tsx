import { useEffect, useRef, type ReactNode } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { CircleAlert, CircleCheck, Info, X } from "lucide-react";
import { TOAST_MS, toast, useToasts, type Toast } from "../state/toasts";

const ICONS = { error: CircleAlert, success: CircleCheck, info: Info };

function ToastItem({ t }: { t: Toast }) {
  const timer = useRef(0);
  const start = () => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => toast.dismiss(t.id), TOAST_MS[t.kind]);
  };
  const stop = () => window.clearTimeout(timer.current);
  useEffect(() => {
    timer.current = window.setTimeout(() => toast.dismiss(t.id), TOAST_MS[t.kind]);
    return () => window.clearTimeout(timer.current);
  }, [t.id, t.kind]);

  const Icon = ICONS[t.kind];
  return (
    <div className={`toast toast-${t.kind}`} role={t.kind === "error" ? "alert" : "status"} onMouseEnter={stop} onMouseLeave={start}>
      <Icon size={18} className="toast-icon" aria-hidden />
      <div className="toast-body">
        <div className="toast-text">{t.text}</div>
        {(t.actions?.length || t.kind === "error") && (
          <div className="toast-actions">
            {t.actions?.map((a) => (
              <button
                key={a.label}
                type="button"
                className="btn btn-ghost btn-sm"
                onClick={() => {
                  a.run();
                  toast.dismiss(t.id);
                }}
              >
                {a.label}
              </button>
            ))}
            {t.kind === "error" && (
              <button type="button" className="btn btn-ghost btn-sm" onClick={() => void writeText(t.text).catch(() => {})}>
                Copy
              </button>
            )}
          </div>
        )}
      </div>
      <button type="button" className="btn btn-icon btn-ghost btn-sm toast-close" aria-label="Dismiss" onClick={() => toast.dismiss(t.id)}>
        <X size={14} />
      </button>
    </div>
  );
}

/** The toast stack, below any persistent `children` such as the update prompt. */
export function Toasts({ children }: { children?: ReactNode }) {
  const toasts = useToasts((s) => s.toasts);
  return (
    <div className="toasts" aria-live="polite">
      {children}
      {toasts.map((t) => (
        <ToastItem key={t.id} t={t} />
      ))}
    </div>
  );
}
