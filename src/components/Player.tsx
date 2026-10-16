import { useEffect, useRef, useState } from "react";
import { commands, type PreviewInfo } from "../bindings";
import { errorMessage, unwrap } from "../lib/ipc";
import { formatTimecode } from "../lib/format";
import { isTyping } from "../lib/keys";
import { MsePlayer, type PlayerStatus } from "../lib/msePlayer";
import { followPlayhead, useView } from "../state/view";

type Props = { id: string };

/** Playhead updates from the video, at most this often. */
const SYNC_INTERVAL_MS = 1000 / 30;
const RATES = [1, 2, 4];

type Load = { kind: "opening" } | { kind: "ready"; info: PreviewInfo } | { kind: "failed"; message: string };

/** Low-res preview video above the timeline, synced with the playhead. */
export function Player({ id }: Props) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const playerRef = useRef<MsePlayer | null>(null);
  const [load, setLoad] = useState<Load>({ kind: "opening" });
  const [status, setStatus] = useState<PlayerStatus>({ kind: "idle" });
  const [playing, setPlaying] = useState(false);
  const [rate, setRate] = useState(1);
  /** J: simulated reverse playback (MSE has no negative rates). 0 = off. */
  const [reverse, setReverse] = useState(0);

  // Open the preview and create the player.
  useEffect(() => {
    // `WaveformPanel` is keyed by session, so `load` starts as "opening" for every id.
    let active = true;
    unwrap(commands.getPreviewInfo(id))
      .then((info) => {
        if (!active || !videoRef.current) return;
        const player = new MsePlayer(videoRef.current, { id, info, onStatus: (s) => active && setStatus(s) });
        playerRef.current = player;
        player.seek(useView.getState().playheadS);
        setLoad({ kind: "ready", info });
      })
      .catch((e: unknown) => active && setLoad({ kind: "failed", message: errorMessage(e) }));
    return () => {
      active = false;
      playerRef.current?.destroy();
      playerRef.current = null;
      useView.getState().setPlaying(false);
      void commands.previewClose(id);
    };
  }, [id]);

  // Seeks from the timeline, spike list and keys.
  useEffect(
    () =>
      useView.subscribe((s, prev) => {
        if (s.seekSeq !== prev.seekSeq) playerRef.current?.seek(s.seekTarget);
        if (s.playSeq !== prev.playSeq) {
          setReverse(0);
          playerRef.current?.play();
        }
      }),
    [],
  );

  // While playing (per animation frame): video time → playhead (throttled), follow mode, and the backend's prefetch.
  useEffect(() => {
    const video = videoRef.current;
    if (!video || !playing) return;
    let handle = 0;
    let lastSync = 0;
    let lastSegment = -1;
    let lastT = video.currentTime;
    const segS = load.kind === "ready" ? load.info.segmentS : 12;
    const tick = (now: number) => {
      if (now - lastSync >= SYNC_INTERVAL_MS) {
        lastSync = now;
        // While a seek loads, the video still shows (or after a reset: is empty at 0 s) the old
        // position; writing that back would undo the click.
        if (playerRef.current?.seeking) {
          schedule();
          return;
        }
        const t = video.currentTime;
        // Loop selection: wrap when playback runs over the out point (not when a seek jumps past it).
        const { loop, inS, outS } = useView.getState();
        const prevT = lastT;
        lastT = t;
        if (loop && inS !== null && outS !== null && prevT < outS && t >= outS && t - prevT < 1) {
          lastT = inS;
          useView.getState().seek(inS);
          schedule();
          return;
        }
        useView.getState().setPlayhead(t);
        followPlayhead();
        const seg = Math.floor(t / segS);
        if (seg !== lastSegment) {
          lastSegment = seg;
          void commands.previewPlayhead(id, t);
        }
      }
      schedule();
    };
    // requestAnimationFrame, not requestVideoFrameCallback: WebKitGTK has the latter but never
    // calls it back for MSE video, so the playhead only moved on pause.
    const schedule = () => {
      handle = requestAnimationFrame(tick);
    };
    schedule();
    return () => {
      cancelAnimationFrame(handle);
      if (!playerRef.current?.seeking) useView.getState().setPlayhead(video.currentTime);
    };
  }, [playing, id, load]);

  // Simulated reverse playback.
  useEffect(() => {
    if (reverse === 0) return;
    const step = 0.1;
    const timer = window.setInterval(() => {
      const video = videoRef.current;
      if (!video) return;
      const t = Math.max(0, video.currentTime - step * reverse);
      useView.getState().seek(t);
      followPlayhead();
      if (t === 0) setReverse(0);
    }, step * 1000);
    return () => window.clearInterval(timer);
  }, [reverse]);

  // Keys: Space, J/K/L, ←/→ (Shift: 5 s), `,`/`.` (one frame).
  useEffect(() => {
    if (load.kind !== "ready") return;
    const fps = load.info.fps;
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.altKey || e.metaKey || isTyping(e.target)) return;
      const video = videoRef.current;
      if (!video) return;
      const jump = (dt: number) => useView.getState().seek(video.currentTime + dt);
      switch (e.key) {
        case " ":
          setReverse(0);
          if (video.paused) void video.play().catch(() => {});
          else video.pause();
          break;
        case "k":
        case "K":
          setReverse(0);
          video.pause();
          break;
        case "l":
        case "L": {
          setReverse(0);
          if (video.paused) {
            video.playbackRate = 1;
            void video.play().catch(() => {});
          } else {
            video.playbackRate = RATES[(RATES.indexOf(video.playbackRate) + 1) % RATES.length] ?? 1;
          }
          break;
        }
        case "j":
        case "J":
          video.pause();
          setReverse((r) => (r === 0 ? 2 : Math.min(r * 2, 8)));
          break;
        case "ArrowLeft":
          jump(e.shiftKey ? -5 : -1);
          break;
        case "ArrowRight":
          jump(e.shiftKey ? 5 : 1);
          break;
        case ",":
          video.pause();
          jump(-1 / fps);
          break;
        case ".":
          video.pause();
          jump(1 / fps);
          break;
        default:
          return;
      }
      e.preventDefault();
    };
    // A focused button would also "click" on Space.
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === " " && !isTyping(e.target)) e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("keyup", onKeyUp);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("keyup", onKeyUp);
    };
  }, [load]);

  const togglePlay = () => {
    const video = videoRef.current;
    if (!video) return;
    setReverse(0);
    if (video.paused) void video.play().catch(() => {});
    else video.pause();
  };

  const playhead = useView((s) => s.playheadS);
  const shownRate = reverse > 0 ? `−${reverse}×` : `${rate}×`;

  return (
    <div className="player">
      <div className="player-video">
        <video
          ref={videoRef}
          playsInline
          onPlay={() => {
            setPlaying(true);
            useView.getState().setPlaying(true);
          }}
          onPause={() => {
            setPlaying(false);
            useView.getState().setPlaying(false);
          }}
          onRateChange={(e) => setRate(e.currentTarget.playbackRate)}
          onClick={togglePlay}
        />
        {load.kind === "opening" && <div className="player-overlay">Preparing preview…</div>}
        {load.kind === "failed" && <div className="player-overlay warn">Preview unavailable: {load.message}</div>}
        {load.kind === "ready" && status.kind === "loading" && <div className="player-overlay">Loading…</div>}
        {status.kind === "error" && <div className="player-overlay warn">{status.message}</div>}
      </div>
      <div className="player-controls">
        <button type="button" onClick={togglePlay} disabled={load.kind !== "ready"} title="Play / pause (Space)">
          {playing ? "Pause" : "Play"}
        </button>
        <span className="mono">{formatTimecode(playhead, 2)}</span>
        <span className="muted">{shownRate}</span>
        <span className="spacer" />
        <span className="muted hint">
          Space play · J/K/L −/stop/+ · ←/→ 1 s (Shift 5 s) · ,/. frame · I/O in/out · Ctrl+L loop
          {load.kind === "ready" && ` · ${load.info.backend}`}
        </span>
      </div>
    </div>
  );
}
