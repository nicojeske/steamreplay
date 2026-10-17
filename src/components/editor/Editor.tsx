import { useState } from "react";
import { VideoOff } from "lucide-react";
import type { PreviewInfo, SessionDetail } from "../../bindings";
import { LIMITS, useUi } from "../../state/ui";
import { Inspector } from "../inspector/Inspector";
import { EmptyState } from "../ui/misc";
import { Splitter } from "../ui/Splitter";
import { Player } from "./Player";
import { TimelinePanel } from "./TimelinePanel";
import { useAnalysis } from "./useAnalysis";

/** Keyed by session id in `App`; `useView` was reset for it before it mounts. */
export function Editor({ detail }: { detail: SessionDetail }) {
  const hasMedia = detail.summary.durationS > 0;
  const analysis = useAnalysis(detail);
  const [preview, setPreview] = useState<PreviewInfo | null>(null);
  const inspectorWidth = useUi((s) => s.inspectorWidth);
  const timelineHeight = useUi((s) => s.timelineHeight);
  const set = useUi((s) => s.set);

  return (
    <div className={`editor ${hasMedia ? "" : "no-media"}`}>
      <div className="stage">
        {hasMedia ? (
          <Player id={detail.summary.id} onInfo={setPreview} />
        ) : (
          <EmptyState icon={<VideoOff size={28} />} title="This recording is empty">
            <p className="muted">Steam has no video chunks for it on disk (any more). The Info tab shows what is left.</p>
          </EmptyState>
        )}
      </div>
      <Splitter
        axis="x"
        invert
        label="Resize the inspector"
        value={inspectorWidth}
        min={LIMITS.inspectorWidth[0]}
        max={LIMITS.inspectorWidth[1]}
        onChange={(v) => set({ inspectorWidth: v })}
      />
      <Inspector detail={detail} analysis={hasMedia ? analysis : null} preview={preview} />
      {hasMedia && (
        <>
          <Splitter
            axis="y"
            invert
            label="Resize the timeline"
            value={timelineHeight}
            min={LIMITS.timelineHeight[0]}
            max={Math.min(LIMITS.timelineHeight[1], Math.round(window.innerHeight * 0.55))}
            onChange={(v) => set({ timelineHeight: v })}
          />
          <TimelinePanel detail={detail} analysis={analysis} />
        </>
      )}
    </div>
  );
}
