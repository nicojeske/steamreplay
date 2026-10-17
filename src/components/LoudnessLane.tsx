import { useEffect, useRef, useState } from "react";
import { commands, type Spike } from "../bindings";
import { unwrap } from "../lib/ipc";
import type { WaveformTiles } from "../lib/waveform";
import { useView } from "../state/view";

type Props = {
  tiles: WaveformTiles;
  thresholdDb: number;
  /** The spikes of the current params: a new list means the backend's curves may have changed. */
  spikes: Spike[];
};

const HEIGHT = 40;
/** The dB range drawn, relative to the baseline. */
const TOP_DB = 40;
const BOTTOM_DB = -10;

type Data = { level: number; b0: number; values: number[] };

/** Loudness above the rolling baseline for the visible range, with the threshold line. */
export function LoudnessLane({ tiles, thresholdDb, spikes }: Props) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [width, setWidth] = useState(0);
  const [data, setData] = useState<Data | null>(null);
  const seq = useRef(0);

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => setWidth(Math.floor(entry.contentRect.width)));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Fetch the visible range shortly after the view stops changing.
  useEffect(() => {
    if (width === 0) return;
    let timer = 0;
    const fetch = () => {
      const { viewStartS, viewEndS } = useView.getState();
      const level = tiles.pickLevel((viewEndS - viewStartS) / width);
      const binS = tiles.binSeconds(level);
      const b0 = Math.max(0, Math.floor(viewStartS / binS));
      const count = Math.ceil((viewEndS - viewStartS) / binS) + 2;
      const n = ++seq.current;
      unwrap(commands.getLoudness(tiles.sessionId, level, b0, count))
        .then((values) => n === seq.current && setData({ level, b0, values }))
        .catch((e: unknown) => console.warn("loudness lane", e));
    };
    const schedule = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(fetch, 50);
    };
    fetch();
    const unsubscribe = useView.subscribe((s, prev) => {
      if (s.viewStartS !== prev.viewStartS || s.viewEndS !== prev.viewEndS) schedule();
    });
    return () => {
      unsubscribe();
      window.clearTimeout(timer);
    };
  }, [tiles, width, spikes]);

  // Draw the data, the threshold line and follow the view between fetches.
  useEffect(() => {
    const draw = () => {
      const canvas = canvasRef.current;
      if (!canvas || width === 0) return;
      const dpr = window.devicePixelRatio || 1;
      canvas.width = Math.round(width * dpr);
      canvas.height = Math.round(HEIGHT * dpr);
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, width, HEIGHT);
      const cs = getComputedStyle(canvas);
      const y = (db: number) => HEIGHT - ((Math.min(Math.max(db, BOTTOM_DB), TOP_DB) - BOTTOM_DB) / (TOP_DB - BOTTOM_DB)) * HEIGHT;
      const { viewStartS, viewEndS } = useView.getState();
      const span = viewEndS - viewStartS;
      if (data && span > 0) {
        const binS = tiles.binSeconds(data.level);
        const toX = (t: number) => ((t - viewStartS) / span) * width;
        ctx.fillStyle = cs.getPropertyValue("--wave").trim();
        for (let i = 0; i * 2 + 1 < data.values.length; i++) {
          const t = (data.b0 + i) * binS;
          const x0 = toX(t);
          const x1 = toX(t + binS);
          if (x1 < 0 || x0 > width) continue;
          const top = y(data.values[i * 2 + 1]);
          const bottom = y(Math.min(0, data.values[i * 2]));
          ctx.fillRect(x0, top, Math.max(1, x1 - x0), Math.max(1, bottom - top));
        }
      }
      ctx.strokeStyle = cs.getPropertyValue("--border").trim();
      ctx.beginPath();
      ctx.moveTo(0, y(0) + 0.5);
      ctx.lineTo(width, y(0) + 0.5);
      ctx.stroke();
      ctx.strokeStyle = cs.getPropertyValue("--spike").trim();
      ctx.beginPath();
      ctx.moveTo(0, Math.round(y(thresholdDb)) + 0.5);
      ctx.lineTo(width, Math.round(y(thresholdDb)) + 0.5);
      ctx.stroke();
    };
    draw();
    let frame = 0;
    const unsubscribe = useView.subscribe(() => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(draw);
    });
    return () => {
      unsubscribe();
      cancelAnimationFrame(frame);
    };
  }, [tiles, data, width, thresholdDb]);

  return (
    <div ref={wrapRef} className="loudness-lane" title={`Loudness above the baseline; the line is the threshold (${thresholdDb.toFixed(1)} dB)`}>
      <canvas ref={canvasRef} style={{ width: "100%", height: HEIGHT }} />
    </div>
  );
}
