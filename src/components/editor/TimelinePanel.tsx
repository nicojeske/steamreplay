import { AudioLines, Crosshair, Maximize2, RotateCcw, ScanSearch, ZoomIn, ZoomOut } from "lucide-react";
import type { SessionDetail } from "../../bindings";
import { formatDuration } from "../../lib/format";
import { useShortcut } from "../../lib/shortcuts";
import { useUi } from "../../state/ui";
import { useView, zoomBy, zoomFit, zoomToRange } from "../../state/view";
import { Button, IconButton } from "../ui/Button";
import { ProgressBar } from "../ui/misc";
import { LoudnessLane } from "./LoudnessLane";
import { Timeline } from "./Timeline";
import type { Analysis } from "./useAnalysis";

const ZOOM_STEP = 1.6;

function zoomSelection() {
  const { inS, outS } = useView.getState();
  if (inS === null || outS === null) return false;
  zoomToRange(inS, outS);
}

/** The timeline with its toolbar; the analysis progress while the waveform is not ready. */
export function TimelinePanel({ detail, analysis }: { detail: SessionDetail; analysis: Analysis }) {
  const { status, tiles, params, spikes } = analysis;
  const follow = useView((s) => s.follow);
  const exportMode = useView((s) => s.exportMode);
  const hasSelection = useView((s) => s.inS !== null);
  const span = useView((s) => s.viewEndS - s.viewStartS);
  const lane = useUi((s) => s.loudnessLane);
  const segS = detail.mpd?.segmentDurationS ?? 3;
  const ready = status.kind === "ready" && tiles !== null && params !== null;

  useShortcut("zoomIn", () => zoomBy(1 / ZOOM_STEP));
  useShortcut("zoomOut", () => zoomBy(ZOOM_STEP));
  useShortcut("zoomFit", zoomFit);
  useShortcut("zoomSelection", zoomSelection);
  useShortcut("follow", () => useView.getState().setFollow(!useView.getState().follow));

  return (
    <section className="timeline-panel" aria-label="Timeline">
      <div className="timeline-toolbar">
        <IconButton label="Zoom out" shortcut="zoomOut" size="sm" icon={<ZoomOut size={16} />} onClick={() => zoomBy(ZOOM_STEP)} />
        <IconButton label="Zoom in" shortcut="zoomIn" size="sm" icon={<ZoomIn size={16} />} onClick={() => zoomBy(1 / ZOOM_STEP)} />
        <IconButton label="Show the whole recording" shortcut="zoomFit" size="sm" icon={<Maximize2 size={15} />} onClick={zoomFit} />
        <IconButton
          label="Zoom to the selection"
          shortcut="zoomSelection"
          size="sm"
          icon={<ScanSearch size={16} />}
          disabled={!hasSelection}
          onClick={zoomSelection}
        />
        <span className="toolbar-sep" />
        <IconButton
          label="Follow the playhead"
          shortcut="follow"
          size="sm"
          icon={<Crosshair size={16} />}
          active={follow}
          onClick={() => useView.getState().setFollow(!follow)}
        />
        <IconButton
          label="Loudness lane: loudness above the baseline, with the threshold line"
          size="sm"
          icon={<AudioLines size={16} />}
          active={lane}
          disabled={!ready}
          onClick={() => useUi.getState().set({ loudnessLane: !lane })}
        />
        <span className="toolbar-text faint num">{span > 0 && `${formatDuration(span, true)} visible`}</span>
        <span className="spacer" />
        {exportMode === "fast" && hasSelection && (
          <span className="toolbar-text faint">Fast export cuts on the {segS} s chunks (dashed lines)</span>
        )}
        <span className="toolbar-text faint hide-narrow">Drag to pan · wheel to zoom · Shift+drag to select · double-click a loud moment to zoom</span>
      </div>

      {ready ? (
        <div className="timeline-body">
          <Timeline id={detail.summary.id} tiles={tiles} spikes={spikes} markers={detail.markers} gaps={status.meta.gaps} segS={segS} />
          {lane && <LoudnessLane tiles={tiles} thresholdDb={params.thresholdDb} spikes={spikes} />}
        </div>
      ) : (
        <div className="timeline-status">
          {status.kind === "running" ? (
            <>
              <AudioLines size={20} className="faint" aria-hidden />
              <span>Analysing audio…</span>
              <ProgressBar value={status.fraction} label="Analysis progress" />
              <Button size="sm" variant="ghost" onClick={analysis.cancel}>
                Cancel
              </Button>
            </>
          ) : (
            <>
              <span className={status.kind === "failed" ? "danger" : "muted"}>
                {status.kind === "failed" ? `Analysis failed: ${status.message}` : "Analysis cancelled."}
              </span>
              <Button size="sm" icon={<RotateCcw size={14} />} onClick={analysis.retry}>
                Analyse again
              </Button>
            </>
          )}
        </div>
      )}
    </section>
  );
}
