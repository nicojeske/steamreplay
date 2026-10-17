import { useEffect, useMemo, useRef } from "react";
import { AudioWaveform, Brackets, Flag, Play, TriangleAlert } from "lucide-react";
import type { Marker, SessionDetail, Spike } from "../../bindings";
import { formatTimecode } from "../../lib/format";
import { useShortcut } from "../../lib/shortcuts";
import { useUi, type MomentsFilter, type MomentsSort } from "../../state/ui";
import { reveal, useView, zoomToRange } from "../../state/view";
import { IconButton } from "../ui/Button";
import { Segmented } from "../ui/Segmented";
import { EmptyState } from "../ui/misc";
import type { Analysis } from "../editor/useAnalysis";
import { Detection } from "./Detection";

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

function playFrom(t: number) {
  const view = useView.getState();
  view.seek(t);
  reveal(t);
  view.requestPlay();
}

/** Props for a clickable row that Enter and Space activate too. */
function activate(run: () => void) {
  return {
    role: "button",
    tabIndex: 0,
    onClick: run,
    onKeyDown: (e: React.KeyboardEvent) => {
      if (e.target !== e.currentTarget || (e.key !== "Enter" && e.key !== " ")) return;
      e.preventDefault();
      run();
    },
  };
}

const MARKER_LABELS: Record<string, string> = {
  usermarker: "Marker",
  error: "Recording error",
  achievement: "Achievement",
};

type Item = { kind: "loud"; rank: number; spike: Spike } | { kind: "marker"; index: number; marker: Marker };

const time = (it: Item) => (it.kind === "loud" ? it.spike.startS : it.marker.videoTimeS);

const FILTERS: { value: MomentsFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "loud", label: "Loud" },
  { value: "markers", label: "Markers" },
];
const SORTS: { value: MomentsSort; label: string }[] = [
  { value: "rank", label: "Rank" },
  { value: "time", label: "Time" },
];

/** Loud moments and Steam markers in one list, with the detection tuning on top. */
export function MomentsTab({ detail, analysis }: { detail: SessionDetail; analysis: Analysis }) {
  const { spikes, params, busy } = analysis;
  const selected = useView((s) => s.selectedSpike);
  const filter = useUi((s) => s.momentsFilter);
  const sort = useUi((s) => s.momentsSort);
  const listRef = useRef<HTMLUListElement>(null);
  const maxScore = useMemo(() => Math.max(1e-6, ...spikes.map((s) => s.score)), [spikes]);

  const items = useMemo(() => {
    const loud: Item[] = filter === "markers" ? [] : spikes.map((spike, rank) => ({ kind: "loud", rank, spike }));
    const markers: Item[] = filter === "loud" ? [] : detail.markers.map((marker, index) => ({ kind: "marker", index, marker }));
    // Rank order: loud moments by rank, then the markers in time order.
    return sort === "rank" ? [...loud, ...markers] : [...loud, ...markers].sort((a, b) => time(a) - time(b));
  }, [spikes, detail.markers, filter, sort]);

  // N/P step by rank, Shift+N/Shift+P in time order, S selects the region.
  const has = spikes.length > 0;
  useShortcut(
    "selectSpike",
    () => {
      const i = useView.getState().selectedSpike;
      if (i === null) return false;
      selectRegion(spikes[i]);
    },
    has,
  );
  const step = (dir: 1 | -1) => {
    const cur = useView.getState().selectedSpike ?? (dir === 1 ? -1 : spikes.length);
    goTo(spikes, Math.min(Math.max(cur + dir, 0), spikes.length - 1));
  };
  const stepInTime = (dir: 1 | -1) => {
    const { playheadS } = useView.getState();
    const eps = 0.01;
    let best = -1;
    spikes.forEach((s, i) => {
      if (dir === 1 && s.startS > playheadS + eps && (best < 0 || s.startS < spikes[best].startS)) best = i;
      if (dir === -1 && s.startS < playheadS - eps && (best < 0 || s.startS > spikes[best].startS)) best = i;
    });
    if (best >= 0) goTo(spikes, best);
  };
  useShortcut("nextSpike", () => step(1), has);
  useShortcut("prevSpike", () => step(-1), has);
  useShortcut("nextSpikeTime", () => stepInTime(1), has);
  useShortcut("prevSpikeTime", () => stepInTime(-1), has);

  useEffect(() => {
    if (selected === null) return;
    listRef.current?.querySelector(`[data-rank="${selected}"]`)?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  const ready = analysis.status.kind === "ready";

  return (
    <div className="moments-tab">
      <Detection
        params={params}
        onChange={analysis.changeParams}
        onReset={analysis.resetParams}
        count={spikes.length}
        busy={busy}
        gameName={detail.summary.gameName}
      />
      <div className="moments-controls">
        <Segmented label="Show" size="sm" value={filter} options={FILTERS} onChange={(f) => useUi.getState().set({ momentsFilter: f })} />
        <span className="spacer" />
        <Segmented label="Sort" size="sm" value={sort} options={SORTS} onChange={(s) => useUi.getState().set({ momentsSort: s })} />
      </div>

      {items.length === 0 ? (
        <EmptyState icon={<AudioWaveform size={22} />} title={ready || filter === "markers" ? "Nothing here" : "Analysing audio…"} className="compact">
          <p className="muted">
            {filter === "markers"
              ? "No Steam markers in this recording."
              : ready
                ? "No loud moments found. Try a higher sensitivity."
                : "Loud moments appear when the analysis is done."}
          </p>
        </EmptyState>
      ) : (
        <ul className="moments" ref={listRef}>
          {items.map((it) =>
            it.kind === "loud" ? (
              <li
                key={`l${it.rank}-${it.spike.startS}`}
                data-rank={it.rank}
                className={`moment loud ${it.rank === selected ? "selected" : ""}`}
                {...activate(() => goTo(spikes, it.rank))}
              >
                <AudioWaveform size={15} className="moment-icon" aria-hidden />
                <span className="moment-rank num">#{it.rank + 1}</span>
                <span className="moment-time mono">{formatTimecode(it.spike.startS)}</span>
                <span className="moment-len num faint">{(it.spike.endS - it.spike.startS).toFixed(1)} s</span>
                <span className="score" title={`Score ${it.spike.score.toFixed(1)}`}>
                  <span style={{ width: `${(it.spike.score / maxScore) * 100}%` }} />
                </span>
                <span className="moment-actions">
                  <IconButton
                    label="Select for export"
                    shortcut="selectSpike"
                    size="sm"
                    icon={<Brackets size={14} />}
                    onClick={(e) => {
                      e.stopPropagation();
                      goTo(spikes, it.rank);
                      selectRegion(it.spike);
                    }}
                  />
                  <IconButton
                    label="Play from here"
                    size="sm"
                    icon={<Play size={14} />}
                    onClick={(e) => {
                      e.stopPropagation();
                      useView.getState().selectSpike(it.rank);
                      playFrom(it.spike.startS);
                    }}
                  />
                </span>
              </li>
            ) : (
              <li
                key={`m${it.index}`}
                className={`moment marker marker-${it.marker.kind}`}
                {...activate(() => {
                  useView.getState().seek(it.marker.videoTimeS);
                  reveal(it.marker.videoTimeS);
                })}
              >
                {it.marker.kind === "error" ? (
                  <TriangleAlert size={15} className="moment-icon" aria-hidden />
                ) : (
                  <Flag size={15} className="moment-icon" aria-hidden />
                )}
                <span className="moment-title truncate" title={it.marker.title || undefined}>
                  {it.marker.title || MARKER_LABELS[it.marker.kind] || it.marker.kind}
                </span>
                <span className="moment-time mono">{formatTimecode(it.marker.videoTimeS)}</span>
                <span className="moment-actions">
                  <IconButton
                    label="Play from here"
                    size="sm"
                    icon={<Play size={14} />}
                    onClick={(e) => {
                      e.stopPropagation();
                      playFrom(it.marker.videoTimeS);
                    }}
                  />
                </span>
              </li>
            ),
          )}
        </ul>
      )}
      {detail.markersOutside > 0 && filter !== "loud" && (
        <p className="moments-footer faint">
          {detail.markersOutside} more marker{detail.markersOutside === 1 ? "" : "s"} outside the video on disk.
        </p>
      )}
    </div>
  );
}
