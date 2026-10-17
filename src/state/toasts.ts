import { create } from "zustand";
import { errorMessage } from "../lib/ipc";

export type ToastKind = "error" | "success" | "info";
export type ToastAction = { label: string; run: () => void };
export type Toast = { id: number; kind: ToastKind; text: string; actions?: ToastAction[] };

/** How long a toast stays, unless hovered. */
export const TOAST_MS: Record<ToastKind, number> = { error: 8000, success: 6000, info: 5000 };

export const useToasts = create<{ toasts: Toast[] }>(() => ({ toasts: [] }));

let nextId = 0;

function push(kind: ToastKind, text: string, actions?: ToastAction[]) {
  const id = nextId++;
  useToasts.setState((s) => ({ toasts: [...s.toasts, { id, kind, text, actions }] }));
  return id;
}

export const toast = {
  error: (e: unknown) => {
    console.warn(e);
    push("error", errorMessage(e));
  },
  success: (text: string, actions?: ToastAction[]) => push("success", text, actions),
  info: (text: string) => push("info", text),
  dismiss: (id: number) => useToasts.setState((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
};
