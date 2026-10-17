import { commands, type DirectInfo, type PreviewInfo } from "../bindings";
import { chunkUrl, previewUrl } from "./replay";

/** Fetch the next segment when less than this is buffered ahead of the playhead. */
const AHEAD_S = 8;
/** Buffered data further behind the playhead is removed. */
const KEEP_BEHIND_S = 60;
/**
 * Stalls in front of a buffered range closer than this plus one segment are jumped over (gaps in
 * the recording: a missing chunk or segment, or the ragged end of a recording).
 */
const MAX_GAP_JUMP_S = 3;

export type PlayerStatus = { kind: "idle" } | { kind: "loading" } | { kind: "error"; message: string };

/** What the player appends. Segment `s` covers video time `[s·segS, (s+1)·segS)`. */
export type Source = {
  kind: "original" | "transcode";
  segS: number;
  count: number;
  /** One SourceBuffer per type. */
  mimes: string[];
  /** Init segment per SourceBuffer, appended once per MediaSource. */
  init: string[] | null;
  /** `SourceBuffer.timestampOffset`. */
  offset: number;
  /** One URL per SourceBuffer, or null for a segment known to be missing. */
  urls: (s: number) => string[] | null;
};

/** Steam's original chunks (M11): one segment per chunk, separate video and audio buffers. */
export function originalSource(id: string, d: DirectInfo): Source {
  const kinds = d.audioMime ? (["v", "a"] as const) : (["v"] as const);
  const missing = new Set(d.missing);
  return {
    kind: "original",
    segS: d.chunkS,
    count: d.count,
    mimes: d.audioMime ? [d.videoMime, d.audioMime] : [d.videoMime],
    init: kinds.map((k) => chunkUrl(id, k, "init")),
    // The chunks carry the time since the recording started.
    offset: -d.v0,
    urls: (s) => (missing.has(d.first + s) ? null : kinds.map((k) => chunkUrl(id, k, d.first + s))),
  };
}

/** Transcoded H.264 segments (M4); they already carry video time. */
export function transcodeSource(id: string, info: PreviewInfo): Source {
  return {
    kind: "transcode",
    segS: info.segmentS,
    count: info.count,
    mimes: [info.mime],
    init: null,
    offset: 0,
    urls: (s) => [previewUrl(id, s)],
  };
}

/** True when this webview can play the original video. */
export function canPlayOriginal(d: DirectInfo) {
  return typeof MediaSource !== "undefined" && [d.videoMime, d.audioMime].every((m) => m === null || MediaSource.isTypeSupported(m));
}

type Options = {
  id: string;
  source: Source;
  onStatus: (s: PlayerStatus) => void;
};

class Stale extends Error {}

/**
 * Plays a `Source` through one MediaSource with one `SourceBuffer` per stream (`segments` mode).
 *
 * Segments are only ever appended in increasing order, as one contiguous run. Appending an
 * earlier segment after a later one made WebKitGTK drop the later segment's first GOP (the
 * segments overlap by one frame), so a seek outside the buffer clears everything and starts a
 * new run.
 */
export class MsePlayer {
  private readonly video: HTMLVideoElement;
  private readonly opts: Options;
  private ms: MediaSource | null = null;
  private sbs: SourceBuffer[] = [];
  /** Init segments, fetched once and appended to every new MediaSource. */
  private initData: Promise<ArrayBuffer[]> | null = null;
  private objectUrl: string | null = null;
  /** Next segment to append in the current run, or null before the first append. */
  private next: number | null = null;
  /** Increases on every reset; async work of an older generation stops. */
  private generation = 0;
  private fetchCtl: AbortController | null = null;
  /** Serialises operations on the SourceBuffer. */
  private chain: Promise<void> = Promise.resolve();
  private loadingNext = false;
  private target: number | null = null;
  /** Play again once the target is loaded (a new MediaSource starts paused). */
  private resume = false;
  private recovered = false;
  private destroyed = false;
  private failedSegment: number | null = null;

  constructor(video: HTMLVideoElement, opts: Options) {
    this.video = video;
    this.opts = opts;
    video.addEventListener("timeupdate", this.onTimeUpdate);
    video.addEventListener("waiting", this.onWaiting);
    video.addEventListener("error", this.onError);
    this.open();
  }

  /** A seek is waiting for its segment; the video time is not meaningful until it lands. */
  get seeking() {
    return this.target !== null;
  }

  get kind() {
    return this.opts.source.kind;
  }

  private get segS() {
    return this.opts.source.segS;
  }

  private get count() {
    return this.opts.source.count;
  }

  private segmentOf(t: number) {
    return Math.min(Math.max(Math.floor(t / this.segS), 0), Math.max(this.count - 1, 0));
  }

  private fetchInit(urls: string[]) {
    this.initData ??= Promise.all(
      urls.map(async (u) => {
        const res = await fetch(u);
        if (!res.ok) throw new Error(`Preview init segment: ${(await res.text()) || res.statusText}`);
        return res.arrayBuffer();
      }),
    );
    // A failed fetch is retried by the next MediaSource.
    this.initData.catch(() => (this.initData = null));
    return this.initData;
  }

  private open() {
    const ms = new MediaSource();
    this.ms = ms;
    this.sbs = [];
    this.next = null;
    if (this.objectUrl) URL.revokeObjectURL(this.objectUrl);
    this.objectUrl = URL.createObjectURL(ms);
    this.video.src = this.objectUrl;
    const { source } = this.opts;
    this.chain = new Promise<void>((resolve) => {
      ms.addEventListener(
        "sourceopen",
        () => {
          try {
            ms.duration = this.count * this.segS;
            this.sbs = source.mimes.map((mime) => {
              const sb = ms.addSourceBuffer(mime);
              sb.mode = "segments";
              sb.timestampOffset = source.offset;
              return sb;
            });
          } catch (e) {
            this.sbs = [];
            this.opts.onStatus({ kind: "error", message: `Preview not supported: ${String(e)}` });
            resolve();
            return;
          }
          if (!source.init) {
            resolve();
            return;
          }
          const sbs = this.sbs;
          this.fetchInit(source.init)
            .then(async (data) => {
              for (const [i, sb] of sbs.entries()) {
                // Not the generation: the first seek bumps it without a new MediaSource.
                if (this.ms !== ms || this.destroyed) throw new Stale();
                await this.op(sb, () => sb.appendBuffer(data[i] as ArrayBuffer));
              }
            })
            .catch(this.ignoreStale)
            .finally(resolve);
        },
        { once: true },
      );
    });
  }

  destroy() {
    this.destroyed = true;
    this.generation++;
    this.fetchCtl?.abort();
    this.video.removeEventListener("timeupdate", this.onTimeUpdate);
    this.video.removeEventListener("waiting", this.onWaiting);
    this.video.removeEventListener("error", this.onError);
    this.video.removeAttribute("src");
    this.video.load();
    if (this.objectUrl) URL.revokeObjectURL(this.objectUrl);
  }

  /** True when `[t, t + 0.5]` (or up to the end) is buffered. */
  private isBuffered(t: number) {
    const b = this.video.buffered;
    for (let i = 0; i < b.length; i++) {
      if (b.start(i) <= t + 0.05 && b.end(i) >= Math.min(t + 0.5, this.count * this.segS - 0.1)) return true;
    }
    return false;
  }

  /**
   * Buffered ranges start where both audio and video have data, and Steam's audio starts a few ms
   * after the video. Setting `currentTime` just before a range made WebKitGTK land seconds off (by
   * the time played since the last seek), so such targets move into the range.
   */
  private snap(t: number) {
    const b = this.video.buffered;
    for (let i = 0; i < b.length; i++) {
      if (t < b.start(i) && b.start(i) - t < 0.25) return b.start(i) + 0.001;
    }
    return t;
  }

  private bufferedEnd(t: number) {
    const b = this.video.buffered;
    for (let i = 0; i < b.length; i++) if (b.start(i) <= t + 0.05 && b.end(i) >= t) return b.end(i);
    return t;
  }

  /** Starts playback, or once the pending seek has landed. */
  play() {
    if (this.destroyed) return;
    if (this.seeking) this.resume = true;
    else void this.video.play().catch(() => {});
  }

  /** Moves the video to `t`, loading the segment first when it is not buffered. */
  seek(t: number) {
    if (this.destroyed) return;
    this.target = t;
    this.failedSegment = null;
    if (this.kind === "transcode") void commands.previewPlayhead(this.opts.id, t);
    if (this.isBuffered(t)) {
      this.video.currentTime = this.snap(t);
      this.target = null;
      this.opts.onStatus({ kind: "idle" });
      this.topUp();
      return;
    }
    const s = this.segmentOf(t);
    this.opts.onStatus({ kind: "loading" });
    if (this.next !== null && s === this.next && this.loadingNext) {
      // The segment right after the buffer is already on its way.
      return;
    }
    if (this.next !== null && s === this.next) {
      this.enqueueAppend(this.generation, s);
      return;
    }
    // Outside the run: start a new one in a fresh MediaSource. Removing all buffered data and
    // reusing the SourceBuffer made WebKitGTK land seconds past the target on the next seek.
    const gen = ++this.generation;
    this.fetchCtl?.abort();
    this.loadingNext = false;
    if (this.next !== null) {
      this.resume = this.resume || !this.video.paused;
      this.open();
    }
    this.next = s;
    this.enqueueAppend(gen, s);
  }

  private ignoreStale = (e: unknown) => {
    if (e instanceof Stale || (e instanceof DOMException && e.name === "AbortError")) return;
    this.opts.onStatus({ kind: "error", message: e instanceof Error ? e.message : String(e) });
  };

  private check(gen: number) {
    if (gen !== this.generation || this.destroyed) throw new Stale();
  }

  private waitIdle(sb: SourceBuffer) {
    if (!sb.updating) return Promise.resolve();
    return new Promise<void>((resolve) => sb.addEventListener("updateend", () => resolve(), { once: true }));
  }

  private async op(sb: SourceBuffer, run: () => void) {
    await this.waitIdle(sb);
    await new Promise<void>((resolve, reject) => {
      const done = () => {
        sb.removeEventListener("error", fail);
        resolve();
      };
      const fail = () => {
        sb.removeEventListener("updateend", done);
        reject(new Error("SourceBuffer error"));
      };
      sb.addEventListener("updateend", done, { once: true });
      sb.addEventListener("error", fail, { once: true });
      try {
        run();
      } catch (e) {
        sb.removeEventListener("updateend", done);
        sb.removeEventListener("error", fail);
        reject(e instanceof Error ? e : new Error(String(e)));
      }
    });
  }

  private enqueueAppend(gen: number, s: number) {
    this.loadingNext = true;
    this.chain = this.chain
      .then(() => this.append(gen, s))
      .catch(this.ignoreStale)
      .finally(() => {
        if (gen === this.generation) this.loadingNext = false;
      });
  }

  private async append(gen: number, s: number) {
    this.check(gen);
    // Only the next segment of the run: guards against a duplicate queued by `topUp`.
    if (s >= this.count || s !== this.next) return;
    const urls = this.opts.source.urls(s);
    const ctl = new AbortController();
    this.fetchCtl = ctl;
    const responses = urls && (await Promise.all(urls.map((u) => fetch(u, { signal: ctl.signal }))));
    this.check(gen);
    if (!responses || responses[0]?.status === 404) {
      // A segment with no chunks on disk: leave a gap and continue after it.
      this.next = s + 1;
      if (this.target !== null && this.segmentOf(this.target) === s) this.target = (s + 1) * this.segS;
      this.enqueueAppend(gen, s + 1);
      return;
    }
    // Index i belongs to SourceBuffer i; a missing audio chunk leaves only that buffer empty.
    const data: (ArrayBuffer | null)[] = [];
    for (const res of responses) {
      if (res.status === 404) {
        data.push(null);
        continue;
      }
      if (!res.ok) {
        const text = await res.text();
        if (res.status === 503) throw new Stale(); // cancelled because the playhead moved away
        this.failedSegment = s;
        throw new Error(`Preview segment ${s}: ${text || res.statusText}`);
      }
      data.push(await res.arrayBuffer());
    }
    this.check(gen);
    if (this.sbs.length === 0) throw new Error("Preview not available");
    for (const [i, sb] of this.sbs.entries()) {
      const d = data[i];
      if (!d) continue;
      try {
        await this.op(sb, () => sb.appendBuffer(d));
      } catch (e) {
        if (!(e instanceof DOMException && e.name === "QuotaExceededError")) throw e;
        // Full: drop everything well behind the playhead and try once more.
        const cut = this.video.currentTime - 5;
        if (cut > 0) await this.op(sb, () => sb.remove(0, cut));
        this.check(gen);
        await this.op(sb, () => sb.appendBuffer(d));
      }
      this.check(gen);
    }
    this.next = s + 1;
    this.failedSegment = null;
    this.recovered = false;
    if (this.next >= this.count && this.ms?.readyState === "open") {
      await Promise.all(this.sbs.map((sb) => this.waitIdle(sb)));
      this.ms.endOfStream();
    }
    if (this.target !== null && this.segmentOf(this.target) === s) {
      this.video.currentTime = this.snap(this.target);
      this.target = null;
      this.opts.onStatus({ kind: "idle" });
      if (this.resume) {
        this.resume = false;
        void this.video.play().catch(() => {});
      }
    }
    // A gap that was not buffered yet when the video stalled ("waiting" fired before this append).
    if (!this.video.paused) this.jumpGap();
    this.topUp();
  }

  /** Appends the next segment when the buffer ahead runs low, and trims the buffer behind. */
  private topUp() {
    if (this.destroyed || this.next === null || this.loadingNext || this.target !== null) return;
    const t = this.video.currentTime;
    if (this.next < this.count && this.next !== this.failedSegment && this.bufferedEnd(t) - t < AHEAD_S) {
      this.enqueueAppend(this.generation, this.next);
    }
    const sbs = this.sbs;
    const b = this.video.buffered;
    if (sbs.length > 0 && b.length > 0 && t - b.start(0) > KEEP_BEHIND_S + this.segS) {
      const gen = this.generation;
      this.chain = this.chain
        .then(async () => {
          for (const sb of sbs) {
            this.check(gen);
            await this.op(sb, () => sb.remove(0, this.video.currentTime - KEEP_BEHIND_S));
          }
        })
        .catch(this.ignoreStale);
    }
  }

  private onTimeUpdate = () => {
    this.topUp();
  };

  /** When the video is stuck in front of a gap, moves it to the buffered range after the gap. */
  private jumpGap() {
    const t = this.video.currentTime;
    if (this.target !== null || this.isBuffered(t)) return true;
    const b = this.video.buffered;
    for (let i = 0; i < b.length; i++) {
      if (b.start(i) > t && b.start(i) - t < MAX_GAP_JUMP_S + this.segS) {
        this.video.currentTime = b.start(i) + 0.01;
        return true;
      }
    }
    return false;
  }

  private onWaiting = () => {
    if (!this.jumpGap()) this.topUp();
  };

  /**
   * A decode error kills the MediaSource; build a new one once and continue where we were.
   * WebKitGTK sometimes fails with "Failed to push buffer" when playback starts while the decoder
   * is still being set up (seen with H.264 and, more often, HEVC); the reload plays on.
   */
  private onError = () => {
    if (this.destroyed || !this.video.error) return;
    const message = this.video.error.message || `media error ${this.video.error.code}`;
    if (this.recovered) {
      this.opts.onStatus({ kind: "error", message });
      return;
    }
    console.warn(`preview: ${message}; reloading`);
    this.recovered = true;
    const t = this.target ?? this.video.currentTime;
    // No "pause" event comes before the error, so this is still what the user wanted.
    this.resume = this.resume || !this.video.paused;
    this.generation++;
    this.fetchCtl?.abort();
    this.loadingNext = false;
    this.open();
    this.seek(t);
  };
}
