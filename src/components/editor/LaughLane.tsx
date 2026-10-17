import { useEffect, useRef, useState } from "react";
import { commands } from "../../bindings";
import { unwrap } from "../../lib/ipc";
import { formatLaughThreshold } from "../../lib/spikeTuning";
import { onThemeChange } from "../../state/ui";
import { useView } from "../../state/view";

type Props = {
  sessionId: string;
  threshold: number;
};

const HEIGHT = 40;
/** Log scale: probabilities this small sit on the floor. The useful thresholds are 0.04–25 %. */
const FLOOR = 1e-4;

/** The laughter probability (one value per second, log scale) for the visible range, with the threshold line (M15). */
export function LaughLane({ sessionId, threshold }: Props) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [width, setWidth] = useState(0);
  // Keyed by session, so the previous session's curve is never drawn.
  const [curve, setCurve] = useState<{ id: string; values: (number | null)[] } | null>(null);

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => setWidth(Math.floor(entry.contentRect.width)));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // The whole curve is small (3 h = 10 800 values): fetched once per session.
  useEffect(() => {
    let active = true;
    unwrap(commands.getLaughCurve(sessionId))
      .then((values) => active && setCurve({ id: sessionId, values }))
      .catch((e: unknown) => console.warn("laugh lane", e));
    return () => {
      active = false;
    };
  }, [sessionId]);

  useEffect(() => {
    const values = curve?.id === sessionId ? curve.values : null;
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
      const y = (p: number) => HEIGHT - (Math.log(Math.min(Math.max(p, FLOOR), 1) / FLOOR) / Math.log(1 / FLOOR)) * (HEIGHT - 2);
      const { viewStartS, viewEndS } = useView.getState();
      const span = viewEndS - viewStartS;
      if (values && span > 0) {
        ctx.fillStyle = cs.getPropertyValue("--wave").trim();
        const sPerPx = span / width;
        if (sPerPx >= 1) {
          // Several seconds per pixel: the max of each column.
          for (let x = 0; x < width; x++) {
            const a = Math.max(0, Math.floor(viewStartS + x * sPerPx));
            const b = Math.min(values.length, Math.ceil(viewStartS + (x + 1) * sPerPx));
            let max = -1;
            for (let i = a; i < b; i++) max = Math.max(max, values[i] ?? 0);
            if (max >= 0) ctx.fillRect(x, y(max), 1, HEIGHT - y(max));
          }
        } else {
          // Zoomed in: one bar per second.
          const toX = (t: number) => ((t - viewStartS) / span) * width;
          const first = Math.max(0, Math.floor(viewStartS));
          const last = Math.min(values.length, Math.ceil(viewEndS));
          for (let i = first; i < last; i++) {
            const x0 = toX(i);
            const v = values[i] ?? 0;
            ctx.fillRect(x0, y(v), Math.max(1, toX(i + 1) - x0), HEIGHT - y(v));
          }
        }
      }
      ctx.strokeStyle = cs.getPropertyValue("--spike").trim();
      ctx.beginPath();
      ctx.moveTo(0, Math.round(y(threshold)) + 0.5);
      ctx.lineTo(width, Math.round(y(threshold)) + 0.5);
      ctx.stroke();
    };
    draw();
    let frame = 0;
    const redraw = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(draw);
    };
    const unsubscribe = useView.subscribe(redraw);
    const unsubscribeTheme = onThemeChange(redraw);
    return () => {
      unsubscribe();
      unsubscribeTheme();
      cancelAnimationFrame(frame);
    };
  }, [sessionId, curve, width, threshold]);

  return (
    <div ref={wrapRef} className="loudness-lane" title={`Laughter probability (log scale); the line is the threshold (${formatLaughThreshold(threshold)})`}>
      <canvas ref={canvasRef} style={{ width: "100%", height: HEIGHT }} />
    </div>
  );
}
