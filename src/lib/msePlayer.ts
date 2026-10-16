import { commands, type PreviewInfo } from "../bindings";
import { previewUrl } from "./replay";

/** Fetch the next segment when less than this is buffered ahead of the playhead. */
const AHEAD_S = 8;
/** Buffered data further behind the playhead is removed. */
const KEEP_BEHIND_S = 60;
/** Stalls in front of a buffered range closer than this are jumped over (gaps in the recording). */
const MAX_GAP_JUMP_S = 3;

export type PlayerStatus = { kind: "idle" } | { kind: "loading" } | { kind: "error"; message: string };

type Options = {
  id: string;
  info: PreviewInfo;
  onStatus: (s: PlayerStatus) => void;
};

class Stale extends Error {}

/**
 * Plays preview segments through one MediaSource `SourceBuffer` (`segments` mode; the segments
 * carry video time, see `media/preview/transcode.rs`).
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
  private sb: SourceBuffer | null = null;
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

  private get segS() {
    return this.opts.info.segmentS;
  }

  private segmentOf(t: number) {
    return Math.min(Math.max(Math.floor(t / this.segS), 0), Math.max(this.opts.info.count - 1, 0));
  }

  private open() {
    const ms = new MediaSource();
    this.ms = ms;
    this.sb = null;
    this.next = null;
    if (this.objectUrl) URL.revokeObjectURL(this.objectUrl);
    this.objectUrl = URL.createObjectURL(ms);
    this.video.src = this.objectUrl;
    this.chain = new Promise<void>((resolve) => {
      ms.addEventListener(
        "sourceopen",
        () => {
          try {
            ms.duration = this.opts.info.count * this.segS;
            const sb = ms.addSourceBuffer(this.opts.info.mime);
            sb.mode = "segments";
            this.sb = sb;
          } catch (e) {
            this.opts.onStatus({ kind: "error", message: `Preview not supported: ${String(e)}` });
          }
          resolve();
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
      if (b.start(i) <= t + 0.05 && b.end(i) >= Math.min(t + 0.5, this.opts.info.count * this.segS - 0.1)) return true;
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
    void commands.previewPlayhead(this.opts.id, t);
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
    if (s >= this.opts.info.count || s !== this.next) return;
    const ctl = new AbortController();
    this.fetchCtl = ctl;
    const res = await fetch(previewUrl(this.opts.id, s), { signal: ctl.signal });
    this.check(gen);
    if (res.status === 404) {
      // A segment with no chunks on disk: leave a gap and continue after it.
      this.next = s + 1;
      if (this.target !== null && this.segmentOf(this.target) === s) this.target = (s + 1) * this.segS;
      this.enqueueAppend(gen, s + 1);
      return;
    }
    if (!res.ok) {
      const text = await res.text();
      if (res.status === 503) throw new Stale(); // cancelled because the playhead moved away
      this.failedSegment = s;
      throw new Error(`Preview segment ${s}: ${text || res.statusText}`);
    }
    const data = await res.arrayBuffer();
    this.check(gen);
    const sb = this.sb;
    if (!sb) throw new Error("Preview not available");
    try {
      await this.op(sb, () => sb.appendBuffer(data));
    } catch (e) {
      if (!(e instanceof DOMException && e.name === "QuotaExceededError")) throw e;
      // Full: drop everything well behind the playhead and try once more.
      const cut = this.video.currentTime - 5;
      if (cut > 0) await this.op(sb, () => sb.remove(0, cut));
      this.check(gen);
      await this.op(sb, () => sb.appendBuffer(data));
    }
    this.check(gen);
    this.next = s + 1;
    this.failedSegment = null;
    this.recovered = false;
    if (this.next >= this.opts.info.count && this.ms?.readyState === "open") {
      await this.waitIdle(sb);
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
    this.topUp();
  }

  /** Appends the next segment when the buffer ahead runs low, and trims the buffer behind. */
  private topUp() {
    if (this.destroyed || this.next === null || this.loadingNext || this.target !== null) return;
    const t = this.video.currentTime;
    if (this.next < this.opts.info.count && this.next !== this.failedSegment && this.bufferedEnd(t) - t < AHEAD_S) {
      this.enqueueAppend(this.generation, this.next);
    }
    const sb = this.sb;
    const b = this.video.buffered;
    if (sb && b.length > 0 && t - b.start(0) > KEEP_BEHIND_S + this.segS) {
      const gen = this.generation;
      this.chain = this.chain
        .then(async () => {
          this.check(gen);
          await this.op(sb, () => sb.remove(0, this.video.currentTime - KEEP_BEHIND_S));
        })
        .catch(this.ignoreStale);
    }
  }

  private onTimeUpdate = () => {
    this.topUp();
  };

  private onWaiting = () => {
    const t = this.video.currentTime;
    if (this.isBuffered(t)) return;
    const b = this.video.buffered;
    for (let i = 0; i < b.length; i++) {
      if (b.start(i) > t && b.start(i) - t < MAX_GAP_JUMP_S) {
        this.video.currentTime = b.start(i) + 0.01;
        return;
      }
    }
    this.topUp();
  };

  /** A decode error kills the MediaSource; build a new one once and continue where we were. */
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
    this.generation++;
    this.fetchCtl?.abort();
    this.loadingNext = false;
    this.open();
    this.seek(t);
  };
}
