import { useEffect, useMemo, useRef } from "react";
import { AudioWaveform, Brackets, Flag, Laugh, Play, RotateCcw, TriangleAlert } from "lucide-react";
import type { Label, Marker, Rating, SessionDetail, Spike } from "../../bindings";
import { formatTimecode } from "../../lib/format";
import { useShortcut } from "../../lib/shortcuts";
import { toast } from "../../state/toasts";
import { useUi, type MomentsFilter, type MomentsSort } from "../../state/ui";
import { reveal, useView, zoomToRange } from "../../state/view";
import { Button, IconButton } from "../ui/Button";
import { Segmented } from "../ui/Segmented";
import { EmptyState, ProgressBar } from "../ui/misc";
import type { Analysis } from "../editor/useAnalysis";
import { Detection } from "./Detection";

/** Half the length of a "missed moment" region around the playhead. */
const MISSED_HALF_S = 3;
const RATING_TEXT: Record<Rating, string> = { good: "Good", meh: "Meh", bad: "Bad", missed: "Missed" };

/** M14 labelling: 1/2/3 rate the selected candidate, then the next unrated one plays; X marks a missed moment. */
function useLabellingKeys(analysis: Analysis) {
  const lab = analysis.labelling;
  const pool = lab?.pool ?? null;
  const rated = useMemo(() => new Set(lab?.labels.map((l) => l.candidateId)), [lab?.labels]);
  const on = pool !== null && pool.length > 0;

  const rate = (rating: Rating) => {
    if (!lab || !pool) return false;
    const { selectedSpike, playheadS } = useView.getState();
    const i = selectedSpike ?? pool.findIndex((c) => c.startS <= playheadS && playheadS <= c.endS);
    const c = pool[i];
    if (!c) return false;
    void lab.saveLabel({ candidateId: c.id, startS: c.startS, endS: c.endS, rating });
    const next = pool.findIndex((d, j) => j > i && !rated.has(d.id));
    if (next >= 0) {
      goTo(analysis.spikes, next);
      useView.getState().requestPlay();
    }
  };
  useShortcut("rateGood", () => rate("good"), on);
  useShortcut("rateMeh", () => rate("meh"), on);
  useShortcut("rateBad", () => rate("bad"), on);
  useShortcut(
    "markMissed",
    () => {
      if (!lab) return false;
      const t = useView.getState().playheadS;
      const label: Label = { candidateId: null, startS: Math.max(0, t - MISSED_HALF_S), endS: t + MISSED_HALF_S, rating: "missed" };
      void lab.saveLabel(label).then(() =>
        toast.success(`Missed moment marked at ${formatTimecode(t)}`, [{ label: "Undo", run: () => void lab.saveLabel(label, true) }]),
      );
    },
    lab !== null,
  );
}

function LabellingHeader({ labels, poolSize }: { labels: Label[]; poolSize: number | null }) {
  if (poolSize === null) {
    return (
      <p className="labelling-header muted">
        Labelling mode: no candidate pool for this recording. Run <span className="mono">experiments/moments/pool.py</span>.
      </p>
    );
  }
  const rated = labels.filter((l) => l.candidateId !== null).length;
  const missed = labels.length - rated;
  return (
    <p className="labelling-header">
      <strong>Labelling</strong>{" "}
      <span className="num">
        {rated} / {poolSize}
      </span>{" "}
      rated{missed > 0 && `, ${missed} missed`}. <span className="faint">1 good · 2 meh · 3 bad · X missed</span>
    </p>
  );
}

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

const filters = (laughs: boolean): { value: MomentsFilter; label: string }[] => [
  { value: "all", label: "All" },
  { value: "loud", label: laughs ? "Laughs" : "Loud" },
  { value: "markers", label: "Markers" },
];

/** Why the list shows loud moments instead of laughs, while that is the case. */
function LaughBanner({ analysis }: { analysis: Analysis }) {
  const { laugh } = analysis;
  if (laugh.kind === "running") {
    return (
      <div className="laugh-banner">
        <Laugh size={15} className="faint" aria-hidden />
        <span>Finding laughs{laugh.fraction !== null && ` ${Math.round(laugh.fraction * 100)} %`}… showing loud moments until then.</span>
        <ProgressBar value={laugh.fraction} label="Laughter analysis progress" />
      </div>
    );
  }
  if (laugh.kind === "unavailable" || laugh.kind === "failed") {
    return (
      <div className="laugh-banner">
        <TriangleAlert size={15} className="warn" aria-hidden />
        <span className="truncate" title={laugh.message}>
          {laugh.kind === "unavailable" ? "Laughter detection unavailable" : "Laughter detection failed"}: showing loud moments.
        </span>
        <Button size="sm" variant="ghost" icon={<RotateCcw size={13} />} onClick={analysis.retryLaugh} title={laugh.message}>
          Retry
        </Button>
      </div>
    );
  }
  return null;
}
const SORTS: { value: MomentsSort; label: string }[] = [
  { value: "rank", label: "Rank" },
  { value: "time", label: "Time" },
];

/** Laughs (or loud moments) and Steam markers in one list, with the detection tuning on top. */
export function MomentsTab({ detail, analysis }: { detail: SessionDetail; analysis: Analysis }) {
  const { spikes, params, busy, detector } = analysis;
  const laughs = detector === "laughter";
  const MomentIcon = laughs ? Laugh : AudioWaveform;
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
  useLabellingKeys(analysis);
  const lab = analysis.labelling;
  const ratingOf = useMemo(() => {
    const byId = new Map(lab?.labels.map((l) => [l.candidateId, l.rating]));
    return (rank: number) => (lab?.pool ? byId.get(lab.pool[rank]?.id) : undefined);
  }, [lab]);

  return (
    <div className="moments-tab">
      {lab ? (
        <LabellingHeader labels={lab.labels} poolSize={lab.pool?.length ?? null} />
      ) : (
        <Detection
          detector={detector}
          params={params}
          onChange={analysis.changeParams}
          onReset={analysis.resetParams}
          count={spikes.length}
          busy={busy}
          gameName={detail.summary.gameName}
        />
      )}
      {!lab && <LaughBanner analysis={analysis} />}
      <div className="moments-controls">
        <Segmented label="Show" size="sm" value={filter} options={filters(laughs)} onChange={(f) => useUi.getState().set({ momentsFilter: f })} />
        <span className="spacer" />
        <Segmented label="Sort" size="sm" value={sort} options={SORTS} onChange={(s) => useUi.getState().set({ momentsSort: s })} />
      </div>

      {items.length === 0 ? (
        <EmptyState icon={<MomentIcon size={22} />} title={ready || filter === "markers" ? "Nothing here" : "Analysing audio…"} className="compact">
          <p className="muted">
            {filter === "markers"
              ? "No Steam markers in this recording."
              : ready
                ? `No ${laughs ? "laughs" : "loud moments"} found. Try a higher sensitivity.`
                : "Moments appear when the analysis is done."}
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
                <MomentIcon size={15} className="moment-icon" aria-hidden />
                <span className="moment-rank num">#{it.rank + 1}</span>
                <span className="moment-time mono">{formatTimecode(it.spike.startS)}</span>
                <span className="moment-len num faint">{(it.spike.endS - it.spike.startS).toFixed(1)} s</span>
                {lab?.pool ? (
                  <RatingChip rating={ratingOf(it.rank)} />
                ) : (
                  <span
                    className="score"
                    title={laughs ? `Laughter ${Math.round(it.spike.score * 100)} %` : `Score ${it.spike.score.toFixed(1)}`}
                  >
                    <span style={{ width: `${(it.spike.score / maxScore) * 100}%` }} />
                  </span>
                )}
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

function RatingChip({ rating }: { rating: Rating | undefined }) {
  return <span className={`rating-chip ${rating ?? "unrated"}`}>{rating ? RATING_TEXT[rating] : "–"}</span>;
}
