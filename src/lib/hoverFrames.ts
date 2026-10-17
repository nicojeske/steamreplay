import { useEffect, useSyncExternalStore } from "react";
import { frameUrl } from "./replay";

/** Frames kept as object URLs per session (144 px JPEGs, ~10 KB each). */
const CAP = 300;
/** Wait this long before fetching, so sweeping across the timeline does not queue every chunk. */
const DEBOUNCE_MS = 60;
/** A failed frame (preview not open yet, ffmpeg error) is retried after this long. */
const RETRY_MS = 5000;

/**
 * Hover frames of one session: an LRU of object URLs, one fetch at a time and one pending
 * request (the latest wins). The backend extracts at most one frame at a time anyway.
 */
class FrameLoader {
  private frames = new Map<number, string>();
  private failed = new Map<number, number>();
  private inFlight: number | null = null;
  private pending: number | null = null;
  private listeners = new Set<() => void>();
  private disposed = false;
  /** Bumped on every change, for `useSyncExternalStore`. */
  version = 0;

  constructor(readonly id: string) {}

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => void this.listeners.delete(listener);
  };

  /** Frame `k` if loaded, else the closest loaded one (`exact: false`) to show meanwhile. */
  lookup(k: number): { src: string; exact: boolean } | null {
    const src = this.frames.get(k);
    if (src) return { src, exact: true };
    let best: string | null = null;
    let bestD = Infinity;
    for (const [j, u] of this.frames) {
      const d = Math.abs(j - k);
      if (d < bestD) [best, bestD] = [u, d];
    }
    return best ? { src: best, exact: false } : null;
  }

  request(k: number) {
    if (this.disposed || this.frames.has(k) || this.inFlight === k) return;
    const failedAt = this.failed.get(k);
    if (failedAt !== undefined && Date.now() - failedAt < RETRY_MS) return;
    if (this.inFlight !== null) {
      this.pending = k;
      return;
    }
    void this.load(k);
  }

  private async load(k: number) {
    this.inFlight = k;
    try {
      const res = await fetch(frameUrl(this.id, k));
      if (!res.ok) throw new Error(`${res.status}`);
      const blob = await res.blob();
      if (this.disposed) return;
      this.frames.set(k, URL.createObjectURL(blob));
      this.failed.delete(k);
      if (this.frames.size > CAP) {
        const [oldest, url] = this.frames.entries().next().value as [number, string];
        URL.revokeObjectURL(url);
        this.frames.delete(oldest);
      }
    } catch {
      this.failed.set(k, Date.now());
    } finally {
      this.inFlight = null;
      const next = this.pending;
      this.pending = null;
      if (!this.disposed) {
        this.version++;
        for (const l of this.listeners) l();
        if (next !== null) this.request(next);
      }
    }
  }

  /** Marks frame `k` as recently used (Map order is the LRU order). */
  touch(k: number) {
    const src = this.frames.get(k);
    if (!src) return;
    this.frames.delete(k);
    this.frames.set(k, src);
  }

  dispose() {
    this.disposed = true;
    for (const url of this.frames.values()) URL.revokeObjectURL(url);
    this.frames.clear();
  }
}

/** Only the open session's frames are kept; opening another one drops the previous loader. */
let current: FrameLoader | null = null;
function loaderFor(id: string): FrameLoader {
  if (current?.id !== id) {
    current?.dispose();
    current = new FrameLoader(id);
  }
  return current;
}

/**
 * The frame to show while hovering time `t` (null: not hovering). Frames exist per chunk
 * (`chunkS`, 3 s), so the image is the start of the chunk under the pointer.
 */
export function useHoverFrame(id: string, t: number | null, chunkS: number): { src: string; exact: boolean } | null {
  const loader = loaderFor(id);
  useSyncExternalStore(loader.subscribe, () => loader.version);
  const k = t === null ? null : Math.max(0, Math.floor(t / chunkS));
  useEffect(() => {
    if (k === null) return;
    loader.touch(k);
    const timer = window.setTimeout(() => loader.request(k), DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [loader, k]);
  return k === null ? null : loader.lookup(k);
}
