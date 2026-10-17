import { useEffect, useRef } from "react";
import type { Spike } from "../bindings";
import { formatTimecode } from "../lib/format";
import { dialogOpen, isTyping } from "../lib/keys";
import { useView, zoomToRange } from "../state/view";

type Props = { spikes: Spike[] };

/** Selects a spike: playhead to its start, zoomed to the region. */
function goTo(spikes: Spike[], i: number) {
  const s = spikes[i];
  if (!s) return;
  const view = useView.getState();
  view.selectSpike(i);
  view.seek(s.startS);
  zoomToRange(s.startS, s.endS);
}

/** Sets the export selection to the spike region. */
function selectRegion(s: Spike | undefined) {
  if (s) useView.getState().setSelection(s.startS, s.endS);
}

/** Ranked loud regions. `N`/`P` step by rank, `Shift+N`/`Shift+P` step in time order, `S` selects the region. */
export function SpikeList({ spikes }: Props) {
  const selected = useView((s) => s.selectedSpike);
  const listRef = useRef<HTMLTableSectionElement>(null);

  // Listed in `SHORTCUTS` (lib/keys.ts).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.altKey || e.metaKey || isTyping(e.target) || dialogOpen() || spikes.length === 0) return;
      const key = e.key.toLowerCase();
      if (key === "s" && !e.shiftKey) {
        const i = useView.getState().selectedSpike;
        if (i === null) return;
        e.preventDefault();
        selectRegion(spikes[i]);
        return;
      }
      if (key !== "n" && key !== "p") return;
      e.preventDefault();
      const { selectedSpike, playheadS } = useView.getState();
      if (!e.shiftKey) {
        const cur = selectedSpike ?? (key === "n" ? -1 : spikes.length);
        goTo(spikes, Math.min(Math.max(cur + (key === "n" ? 1 : -1), 0), spikes.length - 1));
        return;
      }
      const eps = 0.01;
      let best = -1;
      spikes.forEach((s, i) => {
        if (key === "n" && s.startS > playheadS + eps && (best < 0 || s.startS < spikes[best].startS)) best = i;
        if (key === "p" && s.startS < playheadS - eps && (best < 0 || s.startS > spikes[best].startS)) best = i;
      });
      if (best >= 0) goTo(spikes, best);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [spikes]);

  useEffect(() => {
    if (selected === null) return;
    listRef.current?.children[selected]?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  if (spikes.length === 0) return <p className="muted spike-empty">No loud moments found.</p>;

  return (
    <div className="spike-list">
      <table>
        <thead>
          <tr>
            <th>#</th>
            <th>Time</th>
            <th>Length</th>
            <th>Score</th>
            <th />
          </tr>
        </thead>
        <tbody ref={listRef}>
          {spikes.map((s, i) => (
            <tr key={`${s.startS}-${s.endS}`} className={i === selected ? "selected" : undefined} onClick={() => goTo(spikes, i)}>
              <td>{i + 1}</td>
              <td className="mono">{formatTimecode(s.startS)}</td>
              <td>{(s.endS - s.startS).toFixed(1)} s</td>
              <td>{s.score.toFixed(1)}</td>
              <td>
                <button
                  type="button"
                  className="link"
                  title="Use as export selection (S)"
                  onClick={(e) => {
                    e.stopPropagation();
                    goTo(spikes, i);
                    selectRegion(s);
                  }}
                >
                  Select
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="muted hint">N / P: next / previous by rank · Shift: in time order · S: select region for export</p>
    </div>
  );
}
