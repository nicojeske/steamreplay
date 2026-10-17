import { useRef } from "react";

type Props = {
  /** "x": a vertical bar that resizes a width; "y": a horizontal bar that resizes a height. */
  axis: "x" | "y";
  value: number;
  min: number;
  max: number;
  /** The panel grows when the pointer moves left / up (it sits right of / below the bar). */
  invert?: boolean;
  label: string;
  onChange: (value: number) => void;
};

const KEY_STEP = 16;

/** A drag handle between two panels. Arrow keys move it too. */
export function Splitter({ axis, value, min, max, invert, label, onChange }: Props) {
  const drag = useRef<{ start: number; value: number } | null>(null);
  const clamp = (v: number) => Math.round(Math.min(Math.max(v, min), Math.max(min, max)));
  const pos = (e: { clientX: number; clientY: number }) => (axis === "x" ? e.clientX : e.clientY);

  return (
    <div
      className={`splitter splitter-${axis}`}
      role="separator"
      aria-label={label}
      aria-orientation={axis === "x" ? "vertical" : "horizontal"}
      aria-valuenow={value}
      aria-valuemin={min}
      aria-valuemax={max}
      tabIndex={0}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.currentTarget.setPointerCapture(e.pointerId);
        drag.current = { start: pos(e), value };
      }}
      onPointerMove={(e) => {
        const d = drag.current;
        if (!d) return;
        const delta = (pos(e) - d.start) * (invert ? -1 : 1);
        onChange(clamp(d.value + delta));
      }}
      onPointerUp={() => (drag.current = null)}
      onPointerCancel={() => (drag.current = null)}
      onKeyDown={(e) => {
        const grow = axis === "x" ? "ArrowRight" : "ArrowDown";
        const shrink = axis === "x" ? "ArrowLeft" : "ArrowUp";
        if (e.key !== grow && e.key !== shrink) return;
        e.preventDefault();
        const dir = (e.key === grow ? 1 : -1) * (invert ? -1 : 1);
        onChange(clamp(value + dir * KEY_STEP));
      }}
    />
  );
}
