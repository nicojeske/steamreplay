import { useEffect, useMemo, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { commands, type Progress, type SessionDetail, type Spike, type WaveformMeta } from "../bindings";
import { IpcError, unwrap } from "../lib/ipc";
import { WaveformTiles } from "../lib/waveform";
import { useView } from "../state/view";
import { ExportPanel } from "./ExportPanel";
import { Player } from "./Player";
import { Timeline } from "./Timeline";
import { SpikeList } from "./SpikeList";

type Props = { detail: SessionDetail; onError: (e: unknown) => void };

type Status =
  | { kind: "running"; fraction: number | null }
  | { kind: "ready"; meta: WaveformMeta; spikes: Spike[] }
  | { kind: "cancelled" }
  | { kind: "failed"; message: string };

/** Runs the audio analysis for a session and shows the timeline and spike list. */
export function WaveformPanel({ detail, onError }: Props) {
  const id = detail.summary.id;
  const { first, last } = detail.chunks;
  const [status, setStatus] = useState<Status>({ kind: "running", fraction: null });
  const [attempt, setAttempt] = useState(0);
  const jobRef = useRef<string | null>(null);

  useEffect(() => {
    const jobId = crypto.randomUUID();
    jobRef.current = jobId;
    let active = true;
    const progress = new Channel<Progress>();
    progress.onmessage = (p) => active && setStatus({ kind: "running", fraction: p.fraction });
    unwrap(commands.analyzeAudio(id, jobId, progress))
      .then(async (meta) => {
        const spikes = await unwrap(commands.getSpikes(id, null));
        if (!active) return;
        useView.getState().reset(meta.durationS);
        setStatus({ kind: "ready", meta, spikes });
      })
      .catch((e: unknown) => {
        if (!active) return;
        if (e instanceof IpcError && e.kind === "cancelled") {
          setStatus({ kind: "cancelled" });
        } else {
          setStatus({ kind: "failed", message: e instanceof Error ? e.message : String(e) });
          onError(e);
        }
      });
    return () => {
      active = false;
      void commands.cancelJob(jobId);
    };
  }, [id, first, last, attempt, onError]);

  const tiles = useMemo(() => (status.kind === "ready" ? new WaveformTiles(id, status.meta) : null), [id, status]);

  if (status.kind === "running") {
    return (
      <div className="analysis-progress">
        <span>Analysing audio…</span>
        <progress max={1} value={status.fraction ?? undefined} />
        <button type="button" onClick={() => jobRef.current && void commands.cancelJob(jobRef.current)}>
          Cancel
        </button>
      </div>
    );
  }
  if (status.kind !== "ready" || !tiles) {
    return (
      <div className="analysis-progress">
        <span className={status.kind === "failed" ? "warn" : "muted"}>
          {status.kind === "failed" ? `Analysis failed: ${status.message}` : "Analysis cancelled."}
        </span>
        <button
          type="button"
          onClick={() => {
            setStatus({ kind: "running", fraction: null });
            setAttempt((a) => a + 1);
          }}
        >
          Analyse again
        </button>
      </div>
    );
  }
  return (
    <div className="waveform-panel">
      <Player id={id} />
      <Timeline
        tiles={tiles}
        spikes={status.spikes}
        markers={detail.markers}
        gaps={status.meta.gaps}
        segS={detail.mpd?.segmentDurationS ?? 3}
      />
      <ExportPanel id={id} video={detail.mpd?.video ?? null} onError={onError} />
      <h2>
        Loud moments <span className="count">{status.spikes.length}</span>
      </h2>
      <SpikeList spikes={status.spikes} />
    </div>
  );
}
