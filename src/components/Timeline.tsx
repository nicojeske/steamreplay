import { useEffect, useRef, useState } from "react";
import type { Gap, Marker, Spike } from "../bindings";
import { formatTimecode } from "../lib/format";
import { snapSelection } from "../lib/selection";
import type { WaveformTiles } from "../lib/waveform";
import { useView, zoomToRange } from "../state/view";

type Props = {
  tiles: WaveformTiles;
  spikes: Spike[];
  markers: Marker[];
  gaps: Gap[];
  /** Chunk length: the export cuts on these boundaries. */
  segS: number;
};

const OVERVIEW_H = 36;
const RULER_H = 22;
const FLAGS_H = 16;
const MAIN_H = 150;
const HEIGHT = OVERVIEW_H + RULER_H + FLAGS_H + MAIN_H;
const MAIN_TOP = OVERVIEW_H + RULER_H + FLAGS_H;
/** Ruler steps in seconds; the first one that leaves enough room between labels wins. */
const TICK_STEPS = [0.1, 0.2, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600];
const MIN_TICK_PX = 90;
const DRAG_THRESHOLD_PX = 3;
/** How close the pointer must be to a selection edge to grab it. */
const EDGE_GRAB_PX = 5;

type Colors = Record<"wave" | "rms" | "text" | "muted" | "border" | "accent" | "spike" | "playhead" | "warn" | "clip" | "panel" | "selection", string>;

function readColors(el: Element): Colors {
  const cs = getComputedStyle(el);
  const v = (name: string) => cs.getPropertyValue(name).trim();
  return {
    wave: v("--wave"),
    rms: v("--wave-rms"),
    text: v("--text"),
    muted: v("--muted"),
    border: v("--border"),
    accent: v("--accent"),
    spike: v("--spike"),
    playhead: v("--playhead"),
    warn: v("--warn"),
    clip: v("--clip"),
    panel: v("--panel"),
    selection: v("--selection"),
  };
}

function markerColor(kind: string, c: Colors): string {
  if (kind === "usermarker") return c.accent;
  if (kind === "error") return c.warn;
  return c.clip;
}

type Hover = { x: number; y: number; text: string } | null;

type Drag =
  | { kind: "main"; startX: number; startView: [number, number]; moved: boolean }
  | { kind: "overview"; grabOffsetS: number }
  /** Shift+drag makes a new selection; dragging an edge moves it. `anchorS` is the fixed end. */
  | { kind: "select"; anchorS: number };

export function Timeline({ tiles, spikes, markers, gaps, segS }: Props) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [width, setWidth] = useState(0);
  const [hover, setHover] = useState<Hover>(null);
  const drag = useRef<Drag | null>(null);
  const frame = useRef(0);
  // Kept in refs so the draw function and native listeners always see current values.
  const props = useRef({ tiles, spikes, markers, gaps, segS, width });
  useEffect(() => {
    props.current = { tiles, spikes, markers, gaps, segS, width };
  });

  // --- drawing -------------------------------------------------------------------------------

  const draw = useRef(() => {});
  useEffect(() => {
    draw.current = () => {
      const canvas = canvasRef.current;
      const { tiles, spikes, markers, gaps, segS, width: w } = props.current;
      if (!canvas || w === 0) return;
      const dpr = window.devicePixelRatio || 1;
      if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(HEIGHT * dpr)) {
        canvas.width = Math.round(w * dpr);
        canvas.height = Math.round(HEIGHT * dpr);
      }
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      const c = readColors(canvas);
      const { durationS, viewStartS, viewEndS, playheadS, selectedSpike, inS, outS, exportMode } = useView.getState();
      ctx.clearRect(0, 0, w, HEIGHT);
      if (durationS <= 0) return;

      const span = viewEndS - viewStartS;
      const toX = (t: number) => ((t - viewStartS) / span) * w;
      const ovX = (t: number) => (t / durationS) * w;

      // Overview strip: the whole session at the top level.
      drawWave(ctx, tiles, tiles.topLevel, 0, durationS, w, 2, OVERVIEW_H - 4, c, false);
      ctx.fillStyle = c.spike;
      for (const s of spikes) ctx.fillRect(ovX(s.startS), 0, Math.max(1, ovX(s.endS) - ovX(s.startS)), 3);
      if (inS !== null && outS !== null) {
        ctx.fillStyle = c.selection;
        ctx.fillRect(ovX(inS), OVERVIEW_H - 4, Math.max(2, ovX(outS) - ovX(inS)), 3);
      }
      const vx0 = ovX(viewStartS);
      const vx1 = Math.max(vx0 + 2, ovX(viewEndS));
      ctx.fillStyle = "rgb(128 128 128 / 0.18)";
      ctx.fillRect(vx0, 0, vx1 - vx0, OVERVIEW_H);
      ctx.strokeStyle = c.accent;
      ctx.lineWidth = 1;
      ctx.strokeRect(vx0 + 0.5, 0.5, vx1 - vx0 - 1, OVERVIEW_H - 1);
      ctx.fillStyle = c.playhead;
      ctx.fillRect(Math.round(ovX(playheadS)), 0, 1, OVERVIEW_H);
      ctx.fillStyle = c.border;
      ctx.fillRect(0, OVERVIEW_H - 1, w, 1);

      // Ruler.
      const pxPerS = w / span;
      const step = TICK_STEPS.find((s) => s * pxPerS >= MIN_TICK_PX) ?? 3600;
      const decimals = step < 1 ? 1 : 0;
      ctx.font = "11px ui-monospace, Consolas, monospace";
      ctx.textBaseline = "middle";
      ctx.fillStyle = c.muted;
      for (let t = Math.ceil(viewStartS / step) * step; t <= viewEndS; t += step) {
        const x = Math.round(toX(t)) + 0.5;
        ctx.fillRect(x - 0.5, OVERVIEW_H + RULER_H - 6, 1, 6);
        ctx.fillText(formatTimecode(t, decimals), x + 3, OVERVIEW_H + RULER_H / 2 - 1);
      }
      ctx.fillStyle = c.border;
      ctx.fillRect(0, OVERVIEW_H + RULER_H - 1, w, 1);

      // Gaps (hatched).
      ctx.save();
      ctx.strokeStyle = c.muted;
      ctx.globalAlpha = 0.5;
      for (const g of gaps) {
        const x0 = toX(g.startS);
        const x1 = toX(g.endS);
        if (x1 < 0 || x0 > w) continue;
        ctx.save();
        ctx.beginPath();
        ctx.rect(x0, MAIN_TOP, x1 - x0, MAIN_H);
        ctx.clip();
        ctx.beginPath();
        for (let x = x0 - MAIN_H; x < x1; x += 7) {
          ctx.moveTo(x, MAIN_TOP + MAIN_H);
          ctx.lineTo(x + MAIN_H, MAIN_TOP);
        }
        ctx.stroke();
        ctx.restore();
      }
      ctx.restore();

      // Selection: the band and solid in/out edges. A fast export cuts at chunk boundaries, shown
      // as a lighter band with dashed ghost edges.
      if (inS !== null && outS !== null) {
        const top = OVERVIEW_H + RULER_H;
        const h = FLAGS_H + MAIN_H;
        ctx.save();
        ctx.fillStyle = c.selection;
        if (exportMode === "fast") {
          const [g0, g1] = snapSelection(inS, outS, segS, durationS);
          ctx.globalAlpha = 0.08;
          ctx.fillRect(toX(g0), top, toX(g1) - toX(g0), h);
          ctx.globalAlpha = 1;
          ctx.strokeStyle = c.selection;
          ctx.setLineDash([4, 3]);
          for (const t of [g0, g1]) {
            const x = Math.round(toX(t)) + 0.5;
            ctx.beginPath();
            ctx.moveTo(x, top);
            ctx.lineTo(x, top + h);
            ctx.stroke();
          }
          ctx.setLineDash([]);
        }
        ctx.globalAlpha = 0.2;
        ctx.fillRect(toX(inS), top, Math.max(1, toX(outS) - toX(inS)), h);
        ctx.globalAlpha = 1;
        ctx.fillStyle = c.selection;
        for (const t of [inS, outS]) ctx.fillRect(Math.round(toX(t)) - 1, top, 2, h);
        ctx.restore();
      }

      // Spike bands with rank labels.
      ctx.textBaseline = "top";
      spikes.forEach((s, i) => {
        const x0 = toX(s.startS);
        const x1 = toX(s.endS);
        if (x1 < 0 || x0 > w) return;
        ctx.globalAlpha = i === selectedSpike ? 0.32 : 0.16;
        ctx.fillStyle = c.spike;
        ctx.fillRect(x0, MAIN_TOP, Math.max(1, x1 - x0), MAIN_H);
        ctx.globalAlpha = 1;
        if (x1 - x0 > 16 || i < 20) {
          ctx.fillStyle = c.spike;
          ctx.fillText(`#${i + 1}`, Math.max(x0, 0) + 3, MAIN_TOP + 3);
        }
      });

      // Waveform.
      const level = tiles.pickLevel(span / w);
      drawWave(ctx, tiles, level, viewStartS, viewEndS, w, MAIN_TOP + 4, MAIN_H - 8, c, true);

      // Markers.
      for (const m of markers) {
        const x = Math.round(toX(m.videoTimeS)) + 0.5;
        if (x < -10 || x > w + 10) continue;
        const color = markerColor(m.kind, c);
        ctx.strokeStyle = color;
        ctx.fillStyle = color;
        ctx.beginPath();
        ctx.moveTo(x, OVERVIEW_H + RULER_H);
        ctx.lineTo(x, MAIN_TOP + MAIN_H);
        ctx.stroke();
        ctx.beginPath();
        ctx.moveTo(x, OVERVIEW_H + RULER_H + 1);
        ctx.lineTo(x + 10, OVERVIEW_H + RULER_H + 5);
        ctx.lineTo(x, OVERVIEW_H + RULER_H + 10);
        ctx.fill();
        ctx.fillRect(Math.round(ovX(m.videoTimeS)), OVERVIEW_H - 8, 2, 8);
      }

      // Playhead.
      const px = toX(playheadS);
      if (px >= 0 && px <= w) {
        ctx.fillStyle = c.playhead;
        ctx.fillRect(Math.round(px) - 1, OVERVIEW_H + RULER_H, 2, FLAGS_H + MAIN_H);
      }
    };
  }, []);

  const schedule = useRef(() => {
    if (frame.current) return;
    frame.current = requestAnimationFrame(() => {
      frame.current = 0;
      draw.current();
    });
  });

  useEffect(() => {
    schedule.current();
  }, [tiles, spikes, markers, gaps, segS, width]);

  useEffect(() => {
    const redraw = schedule.current;
    const unsubscribeTiles = tiles.subscribe(redraw);
    const unsubscribeView = useView.subscribe(redraw);
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    media.addEventListener("change", redraw);
    return () => {
      unsubscribeTiles();
      unsubscribeView();
      media.removeEventListener("change", redraw);
    };
  }, [tiles]);

  useEffect(() => {
    const el = wrapRef.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => setWidth(Math.floor(entry.contentRect.width)));
    ro.observe(el);
    return () => {
      ro.disconnect();
      cancelAnimationFrame(frame.current);
      frame.current = 0;
    };
  }, []);

  // --- interaction ---------------------------------------------------------------------------

  const timeAtX = (x: number) => {
    const { viewStartS, viewEndS } = useView.getState();
    return viewStartS + (x / Math.max(1, props.current.width)) * (viewEndS - viewStartS);
  };
  const localX = (e: { clientX: number }) => e.clientX - (canvasRef.current?.getBoundingClientRect().left ?? 0);
  const localY = (e: { clientY: number }) => e.clientY - (canvasRef.current?.getBoundingClientRect().top ?? 0);

  // Wheel must be a non-passive native listener to prevent page scrolling.
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const { viewStartS, viewEndS, setView } = useView.getState();
      const w = Math.max(1, props.current.width);
      const span = viewEndS - viewStartS;
      const delta = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
      if (e.shiftKey || Math.abs(e.deltaX) > Math.abs(e.deltaY)) {
        const d = (e.shiftKey ? delta || e.deltaX : e.deltaX) * (span / w);
        useView.getState().setFollow(false);
        setView(viewStartS + d, viewEndS + d);
        return;
      }
      const x = e.clientX - canvas.getBoundingClientRect().left;
      const anchor = viewStartS + (x / w) * span;
      const factor = Math.exp(delta * 0.0015);
      const newSpan = span * factor;
      const start = anchor - (x / w) * newSpan;
      setView(start, start + newSpan);
    };
    canvas.addEventListener("wheel", onWheel, { passive: false });
    return () => canvas.removeEventListener("wheel", onWheel);
  }, []);

  const onPointerDown = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    const { viewStartS, viewEndS, durationS, setView } = useView.getState();
    if (localY(e) < OVERVIEW_H) {
      useView.getState().setFollow(false);
      const t = (localX(e) / Math.max(1, props.current.width)) * durationS;
      if (t < viewStartS || t > viewEndS) {
        const half = (viewEndS - viewStartS) / 2;
        setView(t - half, t + half);
        drag.current = { kind: "overview", grabOffsetS: half };
      } else {
        drag.current = { kind: "overview", grabOffsetS: t - viewStartS };
      }
      return;
    }
    const edge = edgeAt(localX(e), localY(e));
    if (edge !== null) {
      const { inS, outS } = useView.getState();
      drag.current = { kind: "select", anchorS: edge === "in" ? (outS ?? 0) : (inS ?? 0) };
      return;
    }
    if (e.shiftKey) {
      const t = timeAtX(localX(e));
      drag.current = { kind: "select", anchorS: t };
      useView.getState().setSelection(t, t);
      return;
    }
    drag.current = { kind: "main", startX: localX(e), startView: [viewStartS, viewEndS], moved: false };
  };

  /** The selection edge under the pointer, if any (main area only). */
  const edgeAt = (x: number, y: number): "in" | "out" | null => {
    const { inS, outS, viewStartS, viewEndS } = useView.getState();
    if (inS === null || outS === null || y < OVERVIEW_H + RULER_H) return null;
    const toX = (t: number) => ((t - viewStartS) / (viewEndS - viewStartS)) * props.current.width;
    const dIn = Math.abs(toX(inS) - x);
    const dOut = Math.abs(toX(outS) - x);
    if (Math.min(dIn, dOut) > EDGE_GRAB_PX) return null;
    return dIn <= dOut ? "in" : "out";
  };

  const onPointerMove = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const d = drag.current;
    const x = localX(e);
    const w = Math.max(1, props.current.width);
    const { durationS, setView, viewStartS, viewEndS } = useView.getState();
    if (d?.kind === "overview") {
      const t = (x / w) * durationS;
      const span = viewEndS - viewStartS;
      setView(t - d.grabOffsetS, t - d.grabOffsetS + span);
      return;
    }
    if (d?.kind === "select") {
      useView.getState().setSelection(d.anchorS, timeAtX(x));
      return;
    }
    if (d?.kind === "main") {
      if (Math.abs(x - d.startX) > DRAG_THRESHOLD_PX) {
        if (!d.moved) useView.getState().setFollow(false);
        d.moved = true;
      }
      if (d.moved) {
        const [s0, s1] = d.startView;
        const dt = ((d.startX - x) / w) * (s1 - s0);
        setView(s0 + dt, s1 + dt);
      }
      return;
    }
    // Hover: selection edges, marker flags and spike bands.
    const y = localY(e);
    e.currentTarget.style.cursor = edgeAt(x, y) !== null ? "ew-resize" : "";
    let text: string | null = null;
    if (y >= OVERVIEW_H + RULER_H) {
      const t = timeAtX(x);
      const pxS = (viewEndS - viewStartS) / w;
      const m = props.current.markers.find((mk) => Math.abs(mk.videoTimeS - t) <= 5 * pxS);
      if (m) {
        text = `${m.kind}${m.title ? `: ${m.title}` : ""} · ${formatTimecode(m.videoTimeS, 1)}`;
      } else {
        const i = props.current.spikes.findIndex((s) => t >= s.startS && t <= s.endS);
        if (i >= 0) {
          const s = props.current.spikes[i];
          text = `#${i + 1} · ${formatTimecode(s.startS)} · ${(s.endS - s.startS).toFixed(1)} s · score ${s.score.toFixed(1)}`;
        }
      }
    }
    setHover(text ? { x, y, text } : null);
  };

  const onPointerUp = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const d = drag.current;
    drag.current = null;
    if (d?.kind === "main" && !d.moved) useView.getState().seek(timeAtX(localX(e)));
    if (d?.kind === "select") {
      // A Shift+click without dragging leaves an empty selection behind.
      const { inS, outS, clearSelection } = useView.getState();
      if (inS !== null && outS !== null && outS - inS < 0.05) clearSelection();
    }
  };

  const onDoubleClick = (e: React.MouseEvent<HTMLCanvasElement>) => {
    if (localY(e) < OVERVIEW_H) return;
    const t = timeAtX(localX(e));
    const i = props.current.spikes.findIndex((s) => t >= s.startS && t <= s.endS);
    if (i < 0) return;
    const s = props.current.spikes[i];
    useView.getState().selectSpike(i);
    zoomToRange(s.startS, s.endS);
  };

  return (
    <div className="timeline" ref={wrapRef}>
      <canvas
        ref={canvasRef}
        style={{ width: `${width}px`, height: `${HEIGHT}px` }}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={() => (drag.current = null)}
        onPointerLeave={() => setHover(null)}
        onDoubleClick={onDoubleClick}
      />
      {hover && (
        <div className="timeline-tip" style={{ left: Math.min(hover.x + 12, Math.max(0, width - 260)), top: hover.y + 14 }}>
          {hover.text}
        </div>
      )}
    </div>
  );
}

/** Min/max bars per pixel column, with the RMS as a darker inner fill. */
function drawWave(
  ctx: CanvasRenderingContext2D,
  tiles: WaveformTiles,
  level: number,
  t0: number,
  t1: number,
  w: number,
  top: number,
  h: number,
  c: Colors,
  withRms: boolean,
) {
  const mid = top + h / 2;
  const half = h / 2;
  const secPerPx = (t1 - t0) / w;
  ctx.fillStyle = c.wave;
  const rms: [number, number][] = [];
  for (let x = 0; x < w; x++) {
    const s = tiles.summarize(level, t0 + x * secPerPx, t0 + (x + 1) * secPerPx);
    if (!s) continue;
    const yMax = mid - Math.min(1, s.max) * half;
    const yMin = mid - Math.max(-1, s.min) * half;
    ctx.fillRect(x, yMax, 1, Math.max(1, yMin - yMax));
    if (withRms) rms.push([x, Math.min(1, s.rms) * half]);
  }
  if (withRms) {
    ctx.fillStyle = c.rms;
    for (const [x, r] of rms) ctx.fillRect(x, mid - r, 1, Math.max(1, 2 * r));
  }
}
