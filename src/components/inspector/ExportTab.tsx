import { useEffect, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { Collapsible, RadioGroup } from "radix-ui";
import { ChevronDown, FolderOpen, Scissors, Zap, Gauge } from "lucide-react";
import {
  commands,
  type ExportMode,
  type ExportModeKind,
  type ExportPreset,
  type PreciseOpts,
  type SessionDetail,
  type Snap,
  type StreamInfo,
  type VideoCodec,
} from "../../bindings";
import { unwrap } from "../../lib/ipc";
import { formatBytes, formatTimecode } from "../../lib/format";
import { CODEC_LABELS, MIN_VIDEO_KBPS, PRESETS, PRESET_ORDER, isWiderThan16x9, presetLabel, videoKbps } from "../../lib/exportPresets";
import { shortcutKeys, useShortcut } from "../../lib/shortcuts";
import { useExports } from "../../state/exports";
import { toast } from "../../state/toasts";
import { useUi } from "../../state/ui";
import { useView } from "../../state/view";
import { JobRow } from "../exports/JobRow";
import { Button, IconButton } from "../ui/Button";
import { Segmented } from "../ui/Segmented";
import { Callout, Kbd } from "../ui/misc";

const SNAP_DEBOUNCE_MS = 150;
const HEIGHTS = [1440, 1080, 720];
const FRAME_RATES = [60, 30];
const CRF_MIN = 16;
const CRF_MAX = 36;

const MODE_TEXT: Record<ExportModeKind, string> = {
  fast: "Original quality, instant. Cuts on the 3 s chunk boundaries and keeps HEVC, which Discord and some players cannot play.",
  precise: "Cuts on the exact frames and re-encodes. H.264 plays everywhere.",
};

const PRESET_TEXT: Record<ExportPreset, string> = {
  discord: "H.264 · 720p60 · fits 10 MB",
  discordNitro: "H.264 · 1080p60 · fits 500 MB",
  original: "HEVC · source size · high quality",
  custom: "Your own codec, size and target",
};

/** Selection summary, mode and preset, output path, the Export button and this session's exports. */
export function ExportTab({ detail }: { detail: SessionDetail }) {
  const id = detail.summary.id;
  const video = detail.mpd?.video ?? null;
  const inS = useView((s) => s.inS);
  const outS = useView((s) => s.outS);
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
  const jobs = useExports((s) => s.jobs);
  const enqueue = useExports((s) => s.enqueue);

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
        toast.error(e);
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
      toast.error(e);
    }
  };

  const startExport = () => {
    if (inS === null || outS === null || !canExport) return;
    const output = edited && path.trim() ? path.trim() : null;
    const jobMode: ExportMode = precise ? { kind: "precise", ...opts } : { kind: "fast" };
    const label = precise ? `Precise · ${presetLabel(preset)}` : "Fast";
    enqueue({ sessionId: id, gameName: detail.summary.gameName, inS, outS, output, mode: jobMode, label });
    // The next export gets a fresh default name again.
    setEdited(false);
  };

  useShortcut("export", () => {
    if (canExport) startExport();
    // Show why it cannot export.
    else useUi.getState().set({ inspectorTab: "export" });
  });

  const sessionJobs = jobs.filter((j) => j.sessionId === id);

  return (
    <div className="export-tab">
      <section className="card selection-card">
        <div className="card-label">Selection</div>
        {hasSelection ? (
          <>
            <div className="selection-big mono">
              {formatTimecode(inS, 1)} <span className="faint">→</span> {formatTimecode(outS, 1)}
            </div>
            <div className="muted num">
              {durS.toFixed(1)} s
              {!precise && snap && (
                <>
                  {" "}
                  · cut {formatTimecode(snap.actualInS)} – {formatTimecode(snap.actualOutS)} ({(snap.actualOutS - snap.actualInS).toFixed(0)} s) · ~
                  {formatBytes(snap.estBytes)}
                </>
              )}
              {precise && opts.target.kind === "maxSizeMb" && !tooLong && <> · ≤ {opts.target.mb} MB</>}
            </div>
          </>
        ) : (
          <p className="muted">
            Press <Kbd>I</Kbd> and <Kbd>O</Kbd> at the playhead, Shift+drag on the timeline, or use <Scissors size={12} /> Select on a loud
            moment.
          </p>
        )}
      </section>

      <section className="export-section">
        <div className="section-label">Mode</div>
        <Segmented
          label="Export mode"
          value={mode}
          options={[
            {
              value: "fast",
              label: (
                <>
                  <Zap size={14} /> Fast
                </>
              ),
            },
            {
              value: "precise",
              label: (
                <>
                  <Gauge size={14} /> Precise
                </>
              ),
            },
          ]}
          onChange={(m) => useView.getState().setExportMode(m)}
        />
        <p className="hint muted">{MODE_TEXT[mode]}</p>
      </section>

      {precise && (
        <section className="export-section">
          <div className="section-label">Preset</div>
          <RadioGroup.Root className="preset-cards" value={preset} onValueChange={(v) => choosePreset(v as ExportPreset)} aria-label="Preset">
            {PRESET_ORDER.map((p) => (
              <RadioGroup.Item key={p} value={p} className="preset-card">
                <span className="preset-name">{presetLabel(p)}</span>
                <span className="preset-text faint">{PRESET_TEXT[p]}</span>
              </RadioGroup.Item>
            ))}
          </RadioGroup.Root>
          <AdvancedOptions opts={opts} codecs={codecs} video={video} onChange={editOpts} open={preset === "custom"} />
        </section>
      )}

      <section className="export-section">
        <div className="section-label">Output</div>
        <div className="path-row">
          <input
            className="input mono"
            value={path}
            placeholder={hasSelection ? "Output file" : "Chosen when you export"}
            spellCheck={false}
            onChange={(e) => {
              setPath(e.target.value);
              setEdited(true);
            }}
            title={path}
          />
          <IconButton label="Choose file…" variant="secondary" icon={<FolderOpen size={16} />} onClick={() => void browse()} />
        </div>
      </section>

      {tooLong && (
        <Callout tone="warn">
          Too long for {opts.target.kind === "maxSizeMb" ? opts.target.mb : 0} MB ({Math.max(0, kbps ?? 0).toFixed(0)} kbit/s). Shorten the selection
          or lower the resolution.
        </Callout>
      )}
      {codecMissing && <Callout tone="warn">This ffmpeg cannot encode {CODEC_LABELS[opts.codec]}; choose another codec.</Callout>}

      <Button variant="primary" size="lg" className="export-button" icon={<Scissors size={16} />} onClick={startExport} disabled={!canExport}>
        Export clip
        <Kbd>{shortcutKeys("export")}</Kbd>
      </Button>

      {sessionJobs.length > 0 && (
        <section className="export-section">
          <div className="section-label">This recording</div>
          <ul className="job-list">
            {[...sessionJobs].reverse().map((j) => (
              <JobRow key={j.jobId} job={j} />
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}

/** Codec, size, frame rate, crop and target of a precise export. */
function AdvancedOptions({
  opts,
  codecs,
  video,
  onChange,
  open,
}: {
  opts: PreciseOpts;
  codecs: VideoCodec[] | null;
  video: StreamInfo | null;
  onChange: (patch: Partial<PreciseOpts>) => void;
  open: boolean;
}) {
  const offered = codecs ?? [opts.codec];
  const sourceH = video?.height ?? null;
  return (
    <Collapsible.Root className="export-advanced" defaultOpen={open}>
      <Collapsible.Trigger className="link-button">
        Advanced <ChevronDown size={13} className="chevron" aria-hidden />
      </Collapsible.Trigger>
      <Collapsible.Content className="form-grid">
        <label>
          <span>Codec</span>
          <select className="select" value={opts.codec} onChange={(e) => onChange({ codec: e.target.value as VideoCodec })}>
            {offered.map((c) => (
              <option key={c} value={c}>
                {CODEC_LABELS[c]}
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>Height</span>
          <select
            className="select"
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
          <span>Frame rate</span>
          <select className="select" value={opts.fps ?? ""} onChange={(e) => onChange({ fps: e.target.value ? Number(e.target.value) : null })}>
            <option value="">Source</option>
            {FRAME_RATES.map((f) => (
              <option key={f} value={f}>
                {f} fps
              </option>
            ))}
          </select>
        </label>
        <label>
          <span>Target</span>
          <select
            className="select"
            value={opts.target.kind}
            onChange={(e) => onChange({ target: e.target.value === "crf" ? { kind: "crf", q: 22 } : { kind: "maxSizeMb", mb: 10 } })}
          >
            <option value="crf">Quality</option>
            <option value="maxSizeMb">Max size</option>
          </select>
        </label>
        {opts.target.kind === "crf" ? (
          <label className="span-2" title="Lower is better quality and a bigger file">
            <span>Quality {opts.target.q}</span>
            <input
              type="range"
              className="range"
              min={CRF_MIN}
              max={CRF_MAX}
              value={opts.target.q}
              onChange={(e) => onChange({ target: { kind: "crf", q: Number(e.target.value) } })}
            />
          </label>
        ) : (
          <label>
            <span>Max size (MB)</span>
            <input
              className="input"
              type="number"
              min={1}
              value={opts.target.mb}
              onChange={(e) => onChange({ target: { kind: "maxSizeMb", mb: Math.max(1, Math.round(Number(e.target.value)) || 1) } })}
            />
          </label>
        )}
        {isWiderThan16x9(video?.width, video?.height) && (
          <label className="check span-2">
            <input type="checkbox" checked={opts.crop16x9} onChange={(e) => onChange({ crop16x9: e.target.checked })} />
            <span>Crop to 16:9 (centre)</span>
          </label>
        )}
      </Collapsible.Content>
    </Collapsible.Root>
  );
}
