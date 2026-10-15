import { commands, type WaveformMeta } from "../bindings";
import { unwrap } from "./ipc";

/** Bins per tile; tiles are fetched and cached per level. */
export const TILE_BINS = 4096;

export type Summary = { min: number; max: number; rms: number };

/**
 * Fetches waveform bins on demand and caches them as tiles. Drawing asks for ranges with
 * `summarize`; missing tiles are requested and a coarser loaded level is used meanwhile.
 */
export class WaveformTiles {
  private tiles = new Map<string, Float32Array>();
  private pending = new Set<string>();
  private listeners = new Set<() => void>();
  readonly sessionId: string;
  readonly meta: WaveformMeta;

  constructor(sessionId: string, meta: WaveformMeta) {
    this.sessionId = sessionId;
    this.meta = meta;
    // The top level is small (< 2000 bins) and drives the overview strip.
    this.request(this.topLevel, 0);
  }

  get topLevel(): number {
    return this.meta.levels.length - 1;
  }

  binSeconds(level: number): number {
    return this.meta.levels[level].binMs / 1000;
  }

  /** The coarsest level whose bins are no wider than one pixel. */
  pickLevel(secondsPerPx: number): number {
    let level = 0;
    for (let l = 0; l < this.meta.levels.length; l++) {
      if (this.binSeconds(l) <= secondsPerPx) level = l;
    }
    return level;
  }

  subscribe(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private request(level: number, tile: number) {
    const key = `${level}:${tile}`;
    if (this.tiles.has(key) || this.pending.has(key)) return;
    this.pending.add(key);
    unwrap(commands.getWaveform(this.sessionId, level, tile * TILE_BINS, TILE_BINS))
      .then((data) => {
        this.tiles.set(key, Float32Array.from(data));
        this.listeners.forEach((fn) => fn());
      })
      .catch((e: unknown) => console.warn("waveform tile", key, e))
      .finally(() => this.pending.delete(key));
  }

  /**
   * Min, max and RMS over `[t0, t1)` seconds at `level`. Returns `null` while nothing covering
   * the range is loaded yet.
   */
  summarize(level: number, t0: number, t1: number): Summary | null {
    for (let l = level; l < this.meta.levels.length; l++) {
      const r = this.summarizeLevel(l, t0, t1, l === level);
      if (r) return r;
    }
    return null;
  }

  private summarizeLevel(level: number, t0: number, t1: number, fetch: boolean): Summary | null {
    const binS = this.binSeconds(level);
    const bins = this.meta.levels[level].bins;
    const b0 = Math.max(0, Math.floor(t0 / binS));
    const b1 = Math.min(bins, Math.max(b0 + 1, Math.ceil(t1 / binS)));
    if (b0 >= bins) return null;
    let min = Infinity;
    let max = -Infinity;
    let sq = 0;
    let n = 0;
    let complete = true;
    for (let tile = Math.floor(b0 / TILE_BINS); tile * TILE_BINS < b1; tile++) {
      const data = this.tiles.get(`${level}:${tile}`);
      if (!data) {
        if (fetch) this.request(level, tile);
        complete = false;
        continue;
      }
      const from = Math.max(b0, tile * TILE_BINS) - tile * TILE_BINS;
      const to = Math.min(b1, (tile + 1) * TILE_BINS) - tile * TILE_BINS;
      for (let i = from; i < to && i * 3 + 2 < data.length; i++) {
        if (data[i * 3] < min) min = data[i * 3];
        if (data[i * 3 + 1] > max) max = data[i * 3 + 1];
        sq += data[i * 3 + 2] * data[i * 3 + 2];
        n++;
      }
    }
    if (!complete || n === 0) return null;
    return { min, max, rms: Math.sqrt(sq / n) };
  }
}
