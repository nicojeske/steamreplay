import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { commands, type Progress, type SessionDetail, type Spike, type SpikeParams, type WaveformMeta } from "../bindings";
import { IpcError, unwrap } from "../lib/ipc";
import { paramsEqual } from "../lib/spikeTuning";
import { WaveformTiles } from "../lib/waveform";
import { useView } from "../state/view";
import { ExportPanel } from "./ExportPanel";
import { LoudnessLane } from "./LoudnessLane";
import { Player } from "./Player";
import { SpikeTuning } from "./SpikeTuning";
import { Timeline } from "./Timeline";
import { SpikeList } from "./SpikeList";

type Props = { detail: SessionDetail; onError: (e: unknown) => void };

type Status =
  | { kind: "running"; fraction: number | null }
  | { kind: "ready"; meta: WaveformMeta }
  | { kind: "cancelled" }
  | { kind: "failed"; message: string };

/** At most one spike request per this many ms while a slider moves (the last value always runs). */
const SPIKES_THROTTLE_MS = 150;
/** The game's preset is saved this long after the last change. */
const SAVE_DELAY_MS = 1000;

function readFlag(key: string): boolean {
  try {
    return localStorage.getItem(key) === "1";
  } catch {
    return false;
  }
}

function writeFlag(key: string, on: boolean) {
  try {
    localStorage.setItem(key, on ? "1" : "0");
  } catch {
    // Not important: the panel just starts collapsed next time.
  }
}

/** Runs the audio analysis for a session and shows the timeline and spike list. */
export function WaveformPanel({ detail, onError }: Props) {
  const id = detail.summary.id;
  const appId = detail.summary.appId;
  const { first, last } = detail.chunks;
  const [status, setStatus] = useState<Status>({ kind: "running", fraction: null });
  const [attempt, setAttempt] = useState(0);
  const [params, setParams] = useState<SpikeParams | null>(null);
  const [spikes, setSpikes] = useState<Spike[]>([]);
  const [busy, setBusy] = useState(false);
  const [tuneOpen, setTuneOpen] = useState(() => readFlag("spikeTuningOpen"));
  const [lane, setLane] = useState(() => readFlag("loudnessLane"));
  const cacheSeq = useView((s) => s.cacheSeq);
  const jobRef = useRef<string | null>(null);
  const paramsRef = useRef(params);
  useEffect(() => {
    paramsRef.current = params;
  });

  useEffect(() => {
    const jobId = crypto.randomUUID();
    jobRef.current = jobId;
    let active = true;
    const progress = new Channel<Progress>();
    progress.onmessage = (p) => active && setStatus({ kind: "running", fraction: p.fraction });
    unwrap(commands.analyzeAudio(id, jobId, progress))
      .then(async (meta) => {
        const p = await unwrap(commands.getSpikeParams(appId));
        const found = await unwrap(commands.getSpikes(id, p));
        if (!active) return;
        useView.getState().reset(meta.durationS);
        setParams(p);
        setSpikes(found);
        setStatus({ kind: "ready", meta });
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
  }, [id, appId, first, last, attempt, cacheSeq, onError]);

  // --- live re-detection ---------------------------------------------------------------------

  const reqSeq = useRef(0);
  const fetchTimer = useRef(0);
  const lastFetch = useRef(0);
  /** When the first change since the last redraw happened, for the dev timing log. */
  const inputAt = useRef<number | null>(null);

  const fetchSpikes = useCallback(
    (p: SpikeParams) => {
      const run = () => {
        lastFetch.current = performance.now();
        const n = ++reqSeq.current;
        setBusy(true);
        unwrap(commands.getSpikes(id, p))
          .then((found) => {
            if (n !== reqSeq.current) return; // a newer request is on its way
            setSpikes(found);
            setBusy(false);
            // The ranks changed; the old index would point at another region.
            useView.getState().selectSpike(null);
            const t0 = inputAt.current;
            inputAt.current = null;
            if (import.meta.env.DEV && t0 !== null) {
              // Two frames: the timeline redraws in the frame after React commits.
              requestAnimationFrame(() =>
                requestAnimationFrame(() =>
                  console.debug(`spike tuning: ${found.length} spikes, input → redraw ${(performance.now() - t0).toFixed(0)} ms`),
                ),
              );
            }
          })
          .catch((e: unknown) => {
            if (n === reqSeq.current) setBusy(false);
            onError(e);
          });
      };
      window.clearTimeout(fetchTimer.current);
      const wait = lastFetch.current + SPIKES_THROTTLE_MS - performance.now();
      if (wait <= 0) run();
      else fetchTimer.current = window.setTimeout(run, wait);
    },
    [id, onError],
  );

  // Saving: debounced, and flushed when the session closes.
  const pendingSave = useRef<SpikeParams | null>(null);
  const saveTimer = useRef(0);
  const flushSave = useCallback(async () => {
    window.clearTimeout(saveTimer.current);
    const p = pendingSave.current;
    pendingSave.current = null;
    if (p) await unwrap(commands.setSpikeParams(appId, p)).catch(onError);
  }, [appId, onError]);

  useEffect(
    () => () => {
      window.clearTimeout(fetchTimer.current);
      void flushSave();
    },
    [flushSave],
  );

  const changeParams = (p: SpikeParams) => {
    inputAt.current ??= performance.now();
    setParams(p);
    fetchSpikes(p);
    pendingSave.current = p;
    window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => void flushSave(), SAVE_DELAY_MS);
  };

  /** Loads the game's params again (after a reset or a settings change) and re-detects if they changed. */
  const reloadParams = useCallback(async () => {
    try {
      await flushSave();
      const p = await unwrap(commands.getSpikeParams(appId));
      if (paramsRef.current && paramsEqual(paramsRef.current, p)) return;
      setParams(p);
      fetchSpikes(p);
    } catch (e) {
      onError(e);
    }
  }, [appId, fetchSpikes, flushSave, onError]);

  const reset = async () => {
    pendingSave.current = null;
    window.clearTimeout(saveTimer.current);
    try {
      await unwrap(commands.setSpikeParams(appId, null));
    } catch (e) {
      onError(e);
    }
    await reloadParams();
  };

  // The defaults or the game presets may have changed in the settings.
  useEffect(
    () =>
      useView.subscribe((s, prev) => {
        if (s.settingsSeq !== prev.settingsSeq && paramsRef.current) void reloadParams();
      }),
    [reloadParams],
  );

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
  if (status.kind !== "ready" || !tiles || !params) {
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
        spikes={spikes}
        markers={detail.markers}
        gaps={status.meta.gaps}
        segS={detail.mpd?.segmentDurationS ?? 3}
      />
      {lane && <LoudnessLane tiles={tiles} thresholdDb={params.thresholdDb} spikes={spikes} />}
      <ExportPanel id={id} video={detail.mpd?.video ?? null} onError={onError} />
      <h2 className="spike-header">
        Loud moments <span className="count">{spikes.length}</span>
        <button
          type="button"
          className="link"
          aria-expanded={tuneOpen}
          onClick={() => {
            setTuneOpen(!tuneOpen);
            writeFlag("spikeTuningOpen", !tuneOpen);
          }}
        >
          {tuneOpen ? "Hide tuning" : "Tune"}
        </button>
      </h2>
      {tuneOpen && (
        <SpikeTuning
          params={params}
          onChange={changeParams}
          onReset={() => void reset()}
          count={spikes.length}
          busy={busy}
          lane={lane}
          onLane={(on) => {
            setLane(on);
            writeFlag("loudnessLane", on);
          }}
        />
      )}
      <SpikeList spikes={spikes} />
    </div>
  );
}
