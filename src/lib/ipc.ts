import type { AppError } from "../bindings";

type Result<T> = { status: "ok"; data: T } | { status: "error"; error: AppError };

export class IpcError extends Error {
  readonly kind: AppError["kind"];
  constructor(err: AppError) {
    super(err.message);
    this.kind = err.kind;
  }
}

/** Unwraps a tauri-specta result, throwing `IpcError` on `{ status: "error" }`. */
export async function unwrap<T>(result: Promise<Result<T>>): Promise<T> {
  const r = await result;
  if (r.status === "error") throw new IpcError(r.error);
  return r.data;
}

export function errorMessage(e: unknown): string {
  if (e instanceof IpcError) return `${e.kind}: ${e.message}`;
  if (e instanceof Error) return e.message;
  return String(e);
}
