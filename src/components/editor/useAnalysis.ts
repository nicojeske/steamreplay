import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import {
  commands,
  type Label,
  type PoolCandidate,
  type Progress,
  type SessionDetail,
  type Spike,
  type SpikeParams,
  type WaveformMeta,
} from "../../bindings";
import { IpcError, errorMessage, unwrap } from "../../lib/ipc";
import { paramsEqual } from "../../lib/spikeTuning";
import { WaveformTiles } from "../../lib/waveform";
import { toast } from "../../state/toasts";
import { useUi } from "../../state/ui";
import { useView } from "../../state/view";

export type AnalysisStatus =
  | { kind: "running"; fraction: number | null }
  | { kind: "ready"; meta: WaveformMeta }
  | { kind: "cancelled" }
  | { kind: "failed"; message: string };

/** At most one spike request per this many ms while a slider moves (the last value always runs). */
const SPIKES_THROTTLE_MS = 150;
/** The game's preset is saved this long after the last change. */
const SAVE_DELAY_MS = 1000;

export type Analysis = ReturnType<typeof useAnalysis>;

/** Runs the audio analysis of a session and keeps its loud moments in sync with the tuning. */
export function useAnalysis(detail: SessionDetail) {
  const id = detail.summary.id;
  const appId = detail.summary.appId;
  const { first, last } = detail.chunks;
  const hasMedia = detail.summary.durationS > 0;
  const [status, setStatus] = useState<AnalysisStatus>({ kind: "running", fraction: null });
  const [attempt, setAttempt] = useState(0);
  const [params, setParams] = useState<SpikeParams | null>(null);
  const [spikes, setSpikes] = useState<Spike[]>([]);
  const [busy, setBusy] = useState(false);
  const cacheSeq = useView((s) => s.cacheSeq);
  const jobRef = useRef<string | null>(null);
  const paramsRef = useRef(params);
  useEffect(() => {
    paramsRef.current = params;
  });

  useEffect(() => {
    if (!hasMedia) return;
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
        useView.getState().setDuration(meta.durationS);
        setParams(p);
        setSpikes(found);
        setStatus({ kind: "ready", meta });
      })
      .catch((e: unknown) => {
        if (!active) return;
        if (e instanceof IpcError && e.kind === "cancelled") {
          setStatus({ kind: "cancelled" });
        } else {
          setStatus({ kind: "failed", message: errorMessage(e) });
          toast.error(e);
        }
      });
    return () => {
      active = false;
      void commands.cancelJob(jobId);
    };
  }, [id, appId, first, last, attempt, cacheSeq, hasMedia]);

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
            toast.error(e);
          });
      };
      window.clearTimeout(fetchTimer.current);
      const wait = lastFetch.current + SPIKES_THROTTLE_MS - performance.now();
      if (wait <= 0) run();
      else fetchTimer.current = window.setTimeout(run, wait);
    },
    [id],
  );

  // Saving: debounced, and flushed when the session closes.
  const pendingSave = useRef<SpikeParams | null>(null);
  const saveTimer = useRef(0);
  const flushSave = useCallback(async () => {
    window.clearTimeout(saveTimer.current);
    const p = pendingSave.current;
    pendingSave.current = null;
    if (p) await unwrap(commands.setSpikeParams(appId, p)).catch(toast.error);
  }, [appId]);

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
      toast.error(e);
    }
  }, [appId, fetchSpikes, flushSave]);

  const resetParams = async () => {
    pendingSave.current = null;
    window.clearTimeout(saveTimer.current);
    try {
      await unwrap(commands.setSpikeParams(appId, null));
    } catch (e) {
      toast.error(e);
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

  // --- labelling (M14) -----------------------------------------------------------------------

  const labellingOn = useUi((s) => s.labelling);
  // Keyed by session, so a stale result of the previous session is never shown.
  const [lab, setLab] = useState<{ id: string; pool: PoolCandidate[] | null; labels: Label[] } | null>(null);
  useEffect(() => {
    if (!labellingOn) return;
    let active = true;
    Promise.all([unwrap(commands.getLabelPool(id)), unwrap(commands.getLabels(id))]).then(([pool, labels]) => {
      if (active) setLab({ id, pool, labels });
    }, toast.error);
    return () => {
      active = false;
    };
  }, [id, labellingOn]);
  const current = labellingOn && lab?.id === id ? lab : null;
  const pool = current?.pool ?? null;
  const labels = useMemo(() => current?.labels ?? [], [current]);
  // The pool as spikes, in time order, so the timeline and the list show it unchanged.
  const poolSpikes = useMemo(
    () => pool?.map((c): Spike => ({ startS: c.startS, endS: c.endS, peakS: c.startS, peakExcessDb: 0, score: 1 })) ?? null,
    [pool],
  );
  const usingPool = labellingOn && poolSpikes !== null;
  useEffect(() => useView.getState().selectSpike(null), [usingPool]);

  const saveLabel = useCallback(
    async (label: Label, remove = false) => {
      try {
        const labels = await unwrap(commands.setLabel(id, label, remove));
        setLab((l) => (l?.id === id ? { ...l, labels } : l));
      } catch (e) {
        toast.error(e);
      }
    },
    [id],
  );

  return {
    status,
    tiles,
    params,
    spikes: usingPool ? poolSpikes : spikes,
    /** Labelling mode: `pool` is null when the scripts made no pool for this session. */
    labelling: labellingOn ? { pool, labels, saveLabel } : null,
    /** A spike request with the current params is still running. */
    busy,
    changeParams,
    resetParams: () => void resetParams(),
    retry: () => {
      setStatus({ kind: "running", fraction: null });
      setAttempt((a) => a + 1);
    },
    cancel: () => jobRef.current && void commands.cancelJob(jobRef.current),
  };
}
