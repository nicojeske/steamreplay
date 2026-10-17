import { useEffect, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import {
  commands,
  type ExportMode,
  type ExportModeKind,
  type ExportPreset,
  type ExportResult,
  type PreciseOpts,
  type Progress,
  type RecentEntry,
  type Snap,
  type StreamInfo,
  type VideoCodec,
} from "../bindings";
import { IpcError, errorMessage, unwrap } from "../lib/ipc";
import { formatBytes, formatDateTime, formatTimecode } from "../lib/format";
import {
  CODEC_LABELS,
  MIN_VIDEO_KBPS,
  PRESETS,
  PRESET_ORDER,
  isWiderThan16x9,
  presetLabel,
  videoKbps,
} from "../lib/exportPresets";
import { dialogOpen, isTyping } from "../lib/keys";
import { useView } from "../state/view";
import { useExportBusy } from "../state/exportBusy";
import { PathActions } from "./PathActions";

type Props = { id: string; video: StreamInfo | null; onError: (e: unknown) => void };

type JobStatus =
  | { kind: "queued" }
  | { kind: "running"; fraction: number }
  | { kind: "done"; result: ExportResult }
  | { kind: "failed"; message: string }
  | { kind: "cancelled" };

type Job = {
  jobId: string;
  inS: number;
  outS: number;
  /** `null`: the default name, chosen when the job starts (so queued jobs do not collide). */
  output: string | null;
  mode: ExportMode;
  /** "Fast" or "Precise · <preset>". */
  label: string;
  status: JobStatus;
};

const SNAP_DEBOUNCE_MS = 150;
const HEIGHTS = [1440, 1080, 720];
const FRAME_RATES = [60, 30];
const CRF_MIN = 16;
const CRF_MAX = 36;

const TOOLTIPS: Record<ExportModeKind, string> = {
  fast: "Original quality, instant: HEVC copied without re-encoding, cut at the 3 s chunk boundaries. Some apps (Discord, Films & TV without the HEVC extension) cannot play HEVC.",
  precise: "Exact cut on the chosen frames, re-encoded. Plays everywhere with H.264.",
};

/** `I`/`O` set in/out at the playhead, `Esc` clears the selection, `Ctrl+L` toggles loop.
 * Listed in `SHORTCUTS` (lib/keys.ts). */
function useSelectionKeys() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.altKey || e.metaKey || isTyping(e.target) || dialogOpen()) return;
      const view = useView.getState();
      const key = e.key.toLowerCase();
      if (e.ctrlKey) {
        if (key !== "l" || e.shiftKey) return;
        view.setLoop(!view.loop);
      } else if (e.shiftKey) {
        return;
      } else if (key === "i") {
        view.setIn(view.playheadS);
      } else if (key === "o") {
        view.setOut(view.playheadS);
      } else if (e.key === "Escape" && view.inS !== null) {
        view.clearSelection();
      } else {
        return;
      }
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

/** Selection summary, output path, Export button and the list of exports of this session. */
export function ExportPanel({ id, video, onError }: Props) {
  useSelectionKeys();
  const inS = useView((s) => s.inS);
  const outS = useView((s) => s.outS);
  const loop = useView((s) => s.loop);
  const mode = useView((s) => s.exportMode);
  const settingsSeq = useView((s) => s.settingsSeq);
  const [preset, setPreset] = useState<ExportPreset>("discord");
  const [opts, setOpts] = useState<PreciseOpts>(PRESETS.discord.opts);
  const [prefsLoaded, setPrefsLoaded] = useState(false);
  const [codecs, setCodecs] = useState<VideoCodec[] | null>(null);
  const [snap, setSnap] = useState<Snap | null>(null);
  const [path, setPath] = useState("");
  /** The user typed or browsed a path; stop replacing it with the default. */
  const [edited, setEdited] = useState(false);
  const [jobs, setJobs] = useState<Job[]>([]);
  /** Bumped after each finished export, so the recent list reloads. */
  const [recentSeq, setRecentSeq] = useState(0);
  const onErrorRef = useRef(onError);
  useEffect(() => {
    onErrorRef.current = onError;
  });

  // The last used mode and precise options (again when the settings dialog may have changed them).
  useEffect(() => {
    let active = true;
    unwrap(commands.getExportPrefs())
      .then((p) => {
        if (!active) return;
        useView.getState().setExportMode(p.mode ?? "fast");
        if (p.precise) {
          setPreset(p.precise.preset);
          setOpts(p.precise.preset === "custom" ? p.precise.opts : PRESETS[p.precise.preset].opts);
        }
      })
      .catch(() => {})
      .finally(() => active && setPrefsLoaded(true));
    return () => {
      active = false;
    };
  }, [settingsSeq]);

  useEffect(() => {
    if (!prefsLoaded) return;
    unwrap(commands.setExportPrefs({ mode, precise: { preset, opts } })).catch((e: unknown) =>
      console.warn("could not store the export settings", e),
    );
  }, [prefsLoaded, mode, preset, opts]);

  // Codecs this ffmpeg and GPU can encode. Only needed once Precise is chosen (it may detect the GPU).
  const wantCodecs = mode === "precise" && codecs === null;
  useEffect(() => {
    if (!wantCodecs) return;
    let active = true;
    unwrap(commands.getExportCapabilities(id))
      .then((c) => active && setCodecs(c))
      .catch((e: unknown) => {
        if (active) setCodecs([]);
        onErrorRef.current(e);
      });
    return () => {
      active = false;
    };
  }, [id, wantCodecs]);

  // Snapped cut, size estimate and default file name follow the selection.
  useEffect(() => {
    if (inS === null || outS === null) return;
    let active = true;
    const timer = window.setTimeout(() => {
      unwrap(commands.exportSnap(id, inS, outS))
        .then((s) => active && setSnap(s))
        .catch(() => active && setSnap(null));
      if (!edited) {
        unwrap(commands.defaultExportPath(id, inS, mode === "precise"))
          .then((p) => active && setPath(p))
          .catch(() => {});
      }
    }, SNAP_DEBOUNCE_MS);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [id, inS, outS, edited, mode]);

  // Runs queued exports one after another.
  const running = jobs.some((j) => j.status.kind === "running");
  const next = running ? undefined : jobs.find((j) => j.status.kind === "queued");
  const started = useRef(new Set<string>());
  useEffect(() => {
    if (!next || started.current.has(next.jobId)) return;
    started.current.add(next.jobId);
    const update = (status: JobStatus) =>
      setJobs((js) => js.map((j) => (j.jobId === next.jobId ? { ...j, status } : j)));
    update({ kind: "running", fraction: 0 });
    const progress = new Channel<Progress>();
    progress.onmessage = (p) => update({ kind: "running", fraction: p.fraction });
    unwrap(commands.exportClip({ id, inS: next.inS, outS: next.outS, output: next.output, mode: next.mode }, next.jobId, progress))
      .then((result) => {
        update({ kind: "done", result });
        setRecentSeq((n) => n + 1);
      })
      .catch((e: unknown) => {
        if (e instanceof IpcError && e.kind === "cancelled") {
          update({ kind: "cancelled" });
        } else {
          update({ kind: "failed", message: errorMessage(e) });
          onErrorRef.current(e);
        }
      });
  }, [id, next]);

  // The updater waits for the queue to empty before it installs.
  const busy = jobs.some((j) => j.status.kind === "queued" || j.status.kind === "running");
  useEffect(() => {
    useExportBusy.setState({ busy });
  }, [busy]);
  useEffect(() => () => useExportBusy.setState({ busy: false }), []);

  // Leaving the session cancels its exports.
  const jobsRef = useRef(jobs);
  useEffect(() => {
    jobsRef.current = jobs;
  });
  useEffect(
    () => () => {
      for (const j of jobsRef.current) if (j.status.kind === "running") void commands.cancelJob(j.jobId);
    },
    [],
  );

  const hasSelection = inS !== null && outS !== null;
  const durS = hasSelection ? outS - inS : 0;
  const precise = mode === "precise";
  const kbps = opts.target.kind === "maxSizeMb" ? videoKbps(opts.target.mb, durS) : null;
  const tooLong = precise && hasSelection && kbps !== null && kbps < MIN_VIDEO_KBPS;
  const codecMissing = precise && codecs !== null && !codecs.includes(opts.codec);
  const canExport = hasSelection && !tooLong && !codecMissing;

  const choosePreset = (p: ExportPreset) => {
    setPreset(p);
    if (p !== "custom") setOpts(PRESETS[p].opts);
  };
  /** Any change in "Advanced" makes the options custom. */
  const editOpts = (patch: Partial<PreciseOpts>) => {
    setOpts((o) => ({ ...o, ...patch }));
    setPreset("custom");
  };

  const browse = async () => {
    try {
      const chosen = await save({ defaultPath: path || undefined, filters: [{ name: "MP4 video", extensions: ["mp4"] }] });
      if (chosen) {
        setPath(chosen);
        setEdited(true);
      }
    } catch (e) {
      onError(e);
    }
  };

  const startExport = () => {
    if (inS === null || outS === null || !canExport) return;
    const output = edited && path.trim() ? path.trim() : null;
    const jobMode: ExportMode = precise ? { kind: "precise", ...opts } : { kind: "fast" };
    const label = precise ? `Precise · ${presetLabel(preset)}` : "Fast";
    setJobs((js) => [
      ...js,
      { jobId: crypto.randomUUID(), inS, outS, output, mode: jobMode, label, status: { kind: "queued" } },
    ]);
    // The next export gets a fresh default name again.
    setEdited(false);
  };

  const playSelection = () => {
    if (inS === null) return;
    const view = useView.getState();
    view.setLoop(true);
    view.seek(inS);
    view.requestPlay();
  };

  return (
    <div className="export-panel">
      <div className="export-row">
        {hasSelection ? (
          <span className="mono">
            {formatTimecode(inS, 1)} – {formatTimecode(outS, 1)} · {(outS - inS).toFixed(1)} s
            {!precise && snap && (
              <span className="muted">
                {" "}
                → cut {formatTimecode(snap.actualInS)} – {formatTimecode(snap.actualOutS)} ·{" "}
                {(snap.actualOutS - snap.actualInS).toFixed(0)} s · ~{formatBytes(snap.estBytes)}
              </span>
            )}
            {precise && opts.target.kind === "maxSizeMb" && !tooLong && (
              <span className="muted"> · ≤ {opts.target.mb} MB</span>
            )}
          </span>
        ) : (
          <span className="muted">No selection. I / O at the playhead, Shift+drag on the timeline, or “Select” on a loud moment.</span>
        )}
        <span className="spacer" />
        <label className="loop-toggle" title="Loop playback inside the selection (Ctrl+L)">
          <input type="checkbox" checked={loop} onChange={(e) => useView.getState().setLoop(e.target.checked)} /> Loop
        </label>
        <button type="button" onClick={playSelection} disabled={!hasSelection}>
          Play selection
        </button>
        <button type="button" onClick={() => useView.getState().clearSelection()} disabled={!hasSelection} title="Esc">
          Clear
        </button>
      </div>
      <div className="export-row">
        <input
          className="export-path mono"
          value={path}
          placeholder="Output file"
          spellCheck={false}
          onChange={(e) => {
            setPath(e.target.value);
            setEdited(true);
          }}
        />
        <button type="button" onClick={() => void browse()}>
          Browse…
        </button>
        <div className="segmented" role="group" aria-label="Export mode">
          {(["fast", "precise"] as const).map((m) => (
            <button
              key={m}
              type="button"
              aria-pressed={mode === m}
              title={TOOLTIPS[m]}
              onClick={() => useView.getState().setExportMode(m)}
            >
              {m === "fast" ? "Fast" : "Precise"}
            </button>
          ))}
        </div>
        {precise && (
          <select value={preset} onChange={(e) => choosePreset(e.target.value as ExportPreset)} aria-label="Preset">
            {PRESET_ORDER.map((p) => (
              <option key={p} value={p}>
                {presetLabel(p)}
              </option>
            ))}
          </select>
        )}
        <button type="button" className="primary" onClick={startExport} disabled={!canExport} title={TOOLTIPS[mode]}>
          Export
        </button>
      </div>
      {tooLong && (
        <div className="warn">
          Too long for {opts.target.kind === "maxSizeMb" ? opts.target.mb : 0} MB ({Math.max(0, kbps ?? 0).toFixed(0)} kbit/s)
          – shorten the selection or lower the resolution.
        </div>
      )}
      {codecMissing && <div className="warn">This ffmpeg cannot encode {CODEC_LABELS[opts.codec]}; choose another codec.</div>}
      {precise && <AdvancedOptions opts={opts} codecs={codecs} video={video} onChange={editOpts} />}
      {jobs.length > 0 && (
        <ul className="export-jobs">
          {[...jobs].reverse().map((j) => (
            <JobRow key={j.jobId} job={j} onError={onError} />
          ))}
        </ul>
      )}
      <RecentExports seq={recentSeq} onError={onError} />
    </div>
  );
}

/** Codec, size, frame rate, crop and target of a precise export. */
function AdvancedOptions({
  opts,
  codecs,
  video,
  onChange,
}: {
  opts: PreciseOpts;
  codecs: VideoCodec[] | null;
  video: StreamInfo | null;
  onChange: (patch: Partial<PreciseOpts>) => void;
}) {
  const offered = codecs ?? [opts.codec];
  const sourceH = video?.height ?? null;
  return (
    <details className="export-advanced">
      <summary>Advanced</summary>
      <div className="export-advanced-grid">
        <label>
          Codec
          <select value={opts.codec} onChange={(e) => onChange({ codec: e.target.value as VideoCodec })}>
            {offered.map((c) => (
              <option key={c} value={c}>
                {CODEC_LABELS[c]}
              </option>
            ))}
          </select>
        </label>
        <label>
          Height
          <select
            value={opts.maxHeight ?? ""}
            onChange={(e) => onChange({ maxHeight: e.target.value ? Number(e.target.value) : null })}
          >
            <option value="">Source{sourceH ? ` (${sourceH}p)` : ""}</option>
            {HEIGHTS.map((h) => (
              <option key={h} value={h} disabled={sourceH !== null && h >= sourceH}>
                {h}p
              </option>
            ))}
          </select>
        </label>
        <label>
          Frame rate
          <select value={opts.fps ?? ""} onChange={(e) => onChange({ fps: e.target.value ? Number(e.target.value) : null })}>
            <option value="">Source</option>
            {FRAME_RATES.map((f) => (
              <option key={f} value={f}>
                {f} fps
              </option>
            ))}
          </select>
        </label>
        {isWiderThan16x9(video?.width, video?.height) && (
          <label className="loop-toggle">
            <input type="checkbox" checked={opts.crop16x9} onChange={(e) => onChange({ crop16x9: e.target.checked })} /> Crop
            to 16:9
          </label>
        )}
        <label>
          Target
          <select
            value={opts.target.kind}
            onChange={(e) =>
              onChange({ target: e.target.value === "crf" ? { kind: "crf", q: 22 } : { kind: "maxSizeMb", mb: 10 } })
            }
          >
            <option value="crf">Quality</option>
            <option value="maxSizeMb">Max size</option>
          </select>
        </label>
        {opts.target.kind === "crf" ? (
          <label title="Lower is better quality and a bigger file">
            Quality {opts.target.q}
            <input
              type="range"
              min={CRF_MIN}
              max={CRF_MAX}
              value={opts.target.q}
              onChange={(e) => onChange({ target: { kind: "crf", q: Number(e.target.value) } })}
            />
          </label>
        ) : (
          <label>
            MB
            <input
              type="number"
              min={1}
              value={opts.target.mb}
              onChange={(e) => onChange({ target: { kind: "maxSizeMb", mb: Math.max(1, Math.round(Number(e.target.value)) || 1) } })}
            />
          </label>
        )}
      </div>
    </details>
  );
}

function JobRow({ job, onError }: { job: Job; onError: (e: unknown) => void }) {
  const s = job.status;
  const range = `${formatTimecode(job.inS)} – ${formatTimecode(job.outS)}`;
  return (
    <li>
      <span className="mono">{range}</span>
      <span className="muted">{job.label}</span>
      {s.kind === "queued" && <span className="muted">Waiting…</span>}
      {s.kind === "running" && (
        <>
          <progress max={1} value={s.fraction} />
          <button type="button" onClick={() => void commands.cancelJob(job.jobId)}>
            Cancel
          </button>
        </>
      )}
      {s.kind === "done" && (
        <>
          <span className="mono export-file" title={s.result.path}>
            {s.result.path}
          </span>
          <span className="muted">
            {(s.result.actualOutS - s.result.actualInS).toFixed(job.mode.kind === "fast" ? 0 : 1)} s ·{" "}
            {formatBytes(s.result.sizeBytes)} · {s.result.encoder}
          </span>
          {s.result.fellBack && (
            <span className="warn" title="The hardware encoder failed (e.g. the GPU's session limit while Steam records)">
              software fallback
            </span>
          )}
          <PathActions path={s.result.path} onError={onError} />
        </>
      )}
      {s.kind === "failed" && <span className="warn">Failed: {s.message}</span>}
      {s.kind === "cancelled" && <span className="muted">Cancelled</span>}
    </li>
  );
}

/** The last exports of any session, kept across restarts. Missing files are greyed out. */
function RecentExports({ seq, onError }: { seq: number; onError: (e: unknown) => void }) {
  const [entries, setEntries] = useState<RecentEntry[] | null>(null);
  const [reload, setReload] = useState(0);

  useEffect(() => {
    let active = true;
    unwrap(commands.getRecentExports())
      .then((e) => active && setEntries(e))
      .catch((e: unknown) => console.warn("recent exports", e));
    return () => {
      active = false;
    };
  }, [seq, reload]);

  if (!entries?.length) return null;

  const remove = async (path: string) => {
    try {
      await unwrap(commands.removeRecentExport(path));
      setReload((n) => n + 1);
    } catch (e) {
      onError(e);
    }
  };

  return (
    <details className="recent-exports">
      <summary>Recent exports ({entries.length})</summary>
      <ul className="export-jobs">
        {entries.map(({ entry: e, exists }) => (
          <li key={e.path} className={exists ? undefined : "muted"} title={exists ? undefined : "The file is gone or its drive is not connected"}>
            <span>{e.gameName}</span>
            <span className="mono">
              {formatTimecode(e.inS)} – {formatTimecode(e.outS)}
            </span>
            <span className="muted">
              {e.mode === "fast" ? "Fast" : "Precise"} · {formatBytes(e.sizeBytes)} · {formatDateTime(e.finishedAt)}
            </span>
            <span className="mono export-file" title={e.path}>
              {e.path}
            </span>
            {exists ? (
              <PathActions path={e.path} onError={onError} />
            ) : (
              <button type="button" onClick={() => void remove(e.path)}>
                Remove
              </button>
            )}
          </li>
        ))}
      </ul>
    </details>
  );
}
