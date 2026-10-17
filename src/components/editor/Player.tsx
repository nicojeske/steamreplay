import { useEffect, useRef, useState } from "react";
import { RotateCcw, VideoOff } from "lucide-react";
import { commands, type PreviewInfo } from "../../bindings";
import { errorMessage, unwrap } from "../../lib/ipc";
import { canPlayOriginal, MsePlayer, originalSource, transcodeSource, type PlayerStatus } from "../../lib/msePlayer";
import { useShortcut } from "../../lib/shortcuts";
import { followPlayhead, useView } from "../../state/view";
import { Button } from "../ui/Button";
import { Spinner } from "../ui/misc";
import { Transport } from "./Transport";

type Props = {
  id: string;
  /** The preview opened (for the Info tab). */
  onInfo: (info: PreviewInfo | null) => void;
};

/** Playhead updates from the video, at most this often. */
const SYNC_INTERVAL_MS = 1000 / 30;
const RATES = [1, 2, 4];

type Load = { kind: "opening" } | { kind: "ready"; info: PreviewInfo } | { kind: "failed"; message: string };

/** Sessions whose original video failed to play; they use the transcoded preview until restart. */
const fellBack = new Set<string>();

/** The preview video, synced with the playhead, with the transport bar below it. */
export function Player({ id, onInfo }: Props) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const playerRef = useRef<MsePlayer | null>(null);
  const [load, setLoad] = useState<Load>({ kind: "opening" });
  const [attempt, setAttempt] = useState(0);
  const [status, setStatus] = useState<PlayerStatus>({ kind: "idle" });
  const [playing, setPlaying] = useState(false);
  const [rate, setRate] = useState(1);
  /** J: simulated reverse playback (MSE has no negative rates). 0 = off. */
  const [reverse, setReverse] = useState(0);
  const onInfoRef = useRef(onInfo);
  useEffect(() => {
    onInfoRef.current = onInfo;
  });

  // Open the preview and create the player: the original video when the webview can decode it,
  // else (or after it failed to play) the transcoded preview.
  useEffect(() => {
    let active = true;
    const start = (info: PreviewInfo, original: boolean) => {
      const video = videoRef.current;
      if (!active || !video) return;
      const source = original && info.direct ? originalSource(id, info.direct) : transcodeSource(id, info);
      const player = new MsePlayer(video, {
        id,
        source,
        onStatus: (s) => {
          if (!active) return;
          if (s.kind === "error" && player.kind === "original" && info.mode === "auto") {
            console.warn(`preview: original video failed (${s.message}); using the transcoded preview`);
            void fallBack();
            return;
          }
          setStatus(s);
        },
      });
      playerRef.current = player;
      player.seek(useView.getState().playheadS);
      setLoad({ kind: "ready", info });
      onInfoRef.current(info);
    };
    const fallBack = async () => {
      fellBack.add(id);
      playerRef.current?.destroy();
      playerRef.current = null;
      setStatus({ kind: "loading" });
      try {
        const info = await unwrap(commands.previewFallback(id, useView.getState().playheadS));
        setStatus({ kind: "idle" });
        start(info, false);
      } catch (e) {
        if (active) setLoad({ kind: "failed", message: errorMessage(e) });
      }
    };
    unwrap(commands.getPreviewInfo(id))
      .then((info) => {
        const d = info.direct;
        const original = d !== null && (info.mode === "original" || (!fellBack.has(id) && canPlayOriginal(d)));
        if (original || info.backend !== null) start(info, original);
        else void fallBack();
      })
      .catch((e: unknown) => active && setLoad({ kind: "failed", message: errorMessage(e) }));
    return () => {
      active = false;
      playerRef.current?.destroy();
      playerRef.current = null;
      useView.getState().setPlaying(false);
      onInfoRef.current(null);
      void commands.previewClose(id);
    };
  }, [id, attempt]);

  // Seeks from the timeline, the moments list and keys.
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

  const ready = load.kind === "ready";
  const fps = ready ? load.info.fps : 60;

  const togglePlay = () => {
    const video = videoRef.current;
    if (!video) return;
    setReverse(0);
    if (video.paused) void video.play().catch(() => {});
    else video.pause();
  };
  const pause = () => {
    setReverse(0);
    videoRef.current?.pause();
  };
  const playForward = () => {
    const video = videoRef.current;
    if (!video) return;
    setReverse(0);
    if (video.paused) {
      video.playbackRate = 1;
      void video.play().catch(() => {});
    } else {
      video.playbackRate = RATES[(RATES.indexOf(video.playbackRate) + 1) % RATES.length] ?? 1;
    }
  };
  const playBackward = () => {
    videoRef.current?.pause();
    setReverse((r) => (r === 0 ? 2 : Math.min(r * 2, 8)));
  };
  const jump = (dt: number) => {
    const video = videoRef.current;
    if (video) useView.getState().seek(video.currentTime + dt);
  };
  const frame = (dir: 1 | -1) => {
    videoRef.current?.pause();
    jump(dir / fps);
  };

  useShortcut("playPause", togglePlay, ready);
  useShortcut("pause", pause, ready);
  useShortcut("playForward", playForward, ready);
  useShortcut("playBackward", playBackward, ready);
  useShortcut("back1", () => jump(-1), ready);
  useShortcut("forward1", () => jump(1), ready);
  useShortcut("back5", () => jump(-5), ready);
  useShortcut("forward5", () => jump(5), ready);
  useShortcut("prevFrame", () => frame(-1), ready);
  useShortcut("nextFrame", () => frame(1), ready);

  return (
    <div className="player">
      <div className="video-frame">
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
        {load.kind === "opening" && (
          <div className="video-overlay">
            <Spinner size={22} />
            <span>Preparing preview…</span>
          </div>
        )}
        {ready && status.kind === "loading" && (
          <div className="video-overlay subtle">
            <Spinner size={22} />
          </div>
        )}
        {(load.kind === "failed" || status.kind === "error") && (
          <div className="video-overlay error">
            <VideoOff size={26} aria-hidden />
            <span>{load.kind === "failed" ? `Preview unavailable: ${load.message}` : status.kind === "error" && status.message}</span>
            <Button
              size="sm"
              icon={<RotateCcw size={14} />}
              onClick={() => {
                setLoad({ kind: "opening" });
                setStatus({ kind: "idle" });
                setAttempt((a) => a + 1);
              }}
            >
              Retry
            </Button>
          </div>
        )}
      </div>
      <Transport
        ready={ready}
        playing={playing}
        rateLabel={reverse > 0 ? `−${reverse}×` : `${rate}×`}
        onTogglePlay={togglePlay}
        onJump={jump}
        onFrame={frame}
        onRate={playForward}
      />
    </div>
  );
}
