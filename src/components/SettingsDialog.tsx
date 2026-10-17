import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { getVersion } from "@tauri-apps/api/app";
import {
  commands,
  type Backend,
  type CacheSizes,
  type DiscoveryInfo,
  type ExportPreset,
  type ExportPrefs,
  type FfmpegPaths,
  type PreviewSettings,
  type SpikeParams,
} from "../bindings";
import { PRESETS, PRESET_ORDER, presetLabel } from "../lib/exportPresets";
import { formatBytes } from "../lib/format";
import { errorMessage, unwrap } from "../lib/ipc";
import { sensitivityToThreshold, thresholdToSensitivity } from "../lib/spikeTuning";
import { useView } from "../state/view";
import { checkForUpdate, updaterSupported, type Update } from "../lib/updater";

const BACKEND_NAMES: Record<Backend, string> = {
  cuda: "NVIDIA (CUDA / NVENC)",
  d3d11vaAmf: "AMD (D3D11VA / AMF)",
  d3d11vaQsv: "Intel (D3D11VA / Quick Sync)",
  vaapi: "VAAPI (AMD / Intel on Linux)",
  qsv: "Intel Quick Sync",
  software: "Software (CPU)",
};

type Props = {
  info: DiscoveryInfo | null;
  onClose: () => void;
  onChanged: () => void;
  onShowShortcuts: () => void;
  onError: (e: unknown) => void;
  /** A newer release was found; the app shows the install prompt. */
  onUpdate: (update: Update) => void;
};

export function SettingsDialog({ info, onClose, onChanged, onShowShortcuts, onError, onUpdate }: Props) {
  const ref = useRef<HTMLDialogElement>(null);
  const [copied, setCopied] = useState(false);
  const [ffmpeg, setFfmpeg] = useState<FfmpegPaths | string | null>(null);

  useEffect(() => {
    ref.current?.showModal();
    unwrap(commands.getFfmpegInfo()).then(setFfmpeg, (e: unknown) => setFfmpeg(errorMessage(e)));
  }, []);

  const setFfmpegDir = async (path: string | null) => {
    try {
      setFfmpeg(await unwrap(commands.setFfmpegDir(path)));
    } catch (e) {
      onError(e);
    }
  };

  const pickFfmpeg = async () => {
    const dir = await open({ directory: true, title: "Choose the folder with ffmpeg and ffprobe" });
    if (typeof dir === "string") await setFfmpegDir(dir);
  };

  const setOverride = async (path: string | null) => {
    try {
      await unwrap(commands.setRecordingsRootOverride(path));
      onChanged();
    } catch (e) {
      onError(e);
    }
  };

  const pick = async () => {
    const dir = await open({ directory: true, title: "Choose the gamerecordings folder" });
    if (typeof dir === "string") await setOverride(dir);
  };

  const copyDiagnostics = async () => {
    try {
      await writeText(await unwrap(commands.getDiagnostics()));
      setCopied(true);
    } catch (e) {
      onError(e);
    }
  };

  return (
    <dialog ref={ref} className="settings" onClose={onClose}>
      <h2>Settings</h2>

      <h3>Recordings folder override</h3>
      <p className="mono">{info?.recordingsRootOverride ?? "none (automatic)"}</p>
      <div className="buttons">
        <button type="button" onClick={pick}>
          Choose folder…
        </button>
        {info?.recordingsRootOverride && (
          <button type="button" onClick={() => setOverride(null)}>
            Clear override
          </button>
        )}
      </div>

      <h3>Export</h3>
      <ExportSection onError={onError} />

      <h3>Loud parts</h3>
      <SpikeSection onError={onError} />

      <h3>Video preview</h3>
      <PreviewSection onError={onError} />

      <h3>ffmpeg</h3>
      {ffmpeg === null ? (
        <p className="muted">Checking…</p>
      ) : typeof ffmpeg === "string" ? (
        <p className="warn">{ffmpeg}</p>
      ) : (
        <p>
          <span className="mono">{ffmpeg.ffmpeg}</span> ({ffmpeg.source})
          <br />
          <span className="muted">{ffmpeg.version}</span>
        </p>
      )}
      <div className="buttons">
        <button type="button" onClick={pickFfmpeg}>
          Choose ffmpeg folder…
        </button>
        {typeof ffmpeg === "object" && ffmpeg?.source === "settings" && (
          <button type="button" onClick={() => setFfmpegDir(null)}>
            Use bundled / PATH
          </button>
        )}
      </div>

      <h3>Storage</h3>
      <StorageSection onError={onError} />

      <h3>About</h3>
      <AboutSection
        onUpdate={(u) => {
          onUpdate(u);
          ref.current?.close();
        }}
      />

      <h3>Diagnostics</h3>
      <p className="muted">Detected Steam folders</p>
      <ul className="checks">
        {info?.checkedRoots.map((c) => (
          <li key={c.path} className={c.exists ? "ok" : "muted"}>
            {c.exists ? "✓" : "✗"} <span className="mono">{c.path}</span>
            {c.canonical && c.canonical !== c.path && <span className="mono muted"> → {c.canonical}</span>}
          </li>
        ))}
      </ul>
      <p className="muted">Accounts</p>
      {info?.accounts.length ? (
        <ul className="checks">
          {info.accounts.map((a) => (
            <li key={a.accountId + a.recordingsRoot}>
              {a.accountId}: <span className="mono">{a.recordingsRoot}</span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="muted">No account with recordings found.</p>
      )}

      <div className="buttons">
        <button type="button" onClick={copyDiagnostics}>
          {copied ? "Copied ✓" : "Copy diagnostics"}
        </button>
        <button type="button" onClick={onShowShortcuts} title="?">
          Keyboard shortcuts
        </button>
        <span className="spacer" />
        <button type="button" onClick={() => ref.current?.close()}>
          Close
        </button>
      </div>
    </dialog>
  );
}

/** Default export folder, and the mode and preset the next export starts with. */
function ExportSection({ onError }: { onError: (e: unknown) => void }) {
  const [dir, setDir] = useState<string | null | undefined>(undefined);
  const [prefs, setPrefs] = useState<ExportPrefs | null>(null);

  useEffect(() => {
    unwrap(commands.getExportDirOverride()).then(setDir, onError);
    unwrap(commands.getExportPrefs()).then(setPrefs, onError);
  }, [onError]);

  const saveDir = async (path: string | null) => {
    try {
      await unwrap(commands.setExportDirOverride(path));
      setDir(path);
    } catch (e) {
      onError(e);
    }
  };

  const pickDir = async () => {
    const chosen = await open({ directory: true, title: "Choose the export folder" });
    if (typeof chosen === "string") await saveDir(chosen);
  };

  const savePrefs = async (p: ExportPrefs) => {
    setPrefs(p);
    try {
      await unwrap(commands.setExportPrefs(p));
    } catch (e) {
      onError(e);
    }
  };

  const mode = prefs?.mode ?? "fast";
  const preset = prefs?.precise?.preset ?? "discord";
  const choosePreset = (p: ExportPreset) => {
    if (!prefs) return;
    const opts = p === "custom" ? (prefs.precise?.opts ?? PRESETS.discord.opts) : PRESETS[p].opts;
    void savePrefs({ ...prefs, precise: { preset: p, opts } });
  };

  return (
    <>
      <p>
        Folder: <span className="mono">{dir ?? "Steam's export folder (automatic)"}</span>
      </p>
      <div className="buttons">
        <button type="button" onClick={() => void pickDir()}>
          Choose folder…
        </button>
        {dir && (
          <button type="button" onClick={() => void saveDir(null)}>
            Use Steam's folder
          </button>
        )}
      </div>
      {prefs && (
        <div className="buttons">
          <span>Next export</span>
          <div className="segmented" role="group" aria-label="Export mode">
            {(["fast", "precise"] as const).map((m) => (
              <button key={m} type="button" aria-pressed={mode === m} onClick={() => void savePrefs({ ...prefs, mode: m })}>
                {m === "fast" ? "Fast" : "Precise"}
              </button>
            ))}
          </div>
          <select
            value={preset}
            disabled={mode !== "precise"}
            onChange={(e) => choosePreset(e.target.value as ExportPreset)}
            aria-label="Preset"
          >
            {PRESET_ORDER.map((p) => (
              <option key={p} value={p}>
                {presetLabel(p)}
              </option>
            ))}
          </select>
        </div>
      )}
      <p className="muted">The export panel remembers the last choice; this edits the same setting.</p>
    </>
  );
}

/** Default sensitivity for games without their own tuning, and resetting the game presets. */
function SpikeSection({ onError }: { onError: (e: unknown) => void }) {
  const [defaults, setDefaults] = useState<SpikeParams | null>(null);
  const [cleared, setCleared] = useState(false);

  useEffect(() => {
    unwrap(commands.getSpikeDefaults()).then(setDefaults, onError);
  }, [onError]);

  const save = async (p: SpikeParams | null) => {
    try {
      await unwrap(commands.setSpikeDefaults(p));
      setDefaults(await unwrap(commands.getSpikeDefaults()));
    } catch (e) {
      onError(e);
    }
  };

  const clearPresets = async () => {
    try {
      await unwrap(commands.clearSpikePresets());
      setCleared(true);
    } catch (e) {
      onError(e);
    }
  };

  if (!defaults) return <p className="muted">Loading…</p>;
  const sensitivity = Math.round(thresholdToSensitivity(defaults.thresholdDb));
  return (
    <>
      <div className="buttons">
        <label className="spike-slider">
          <span>Default sensitivity</span>
          <input
            type="range"
            min={0}
            max={100}
            step={1}
            value={sensitivity}
            onChange={(e) => setDefaults({ ...defaults, thresholdDb: sensitivityToThreshold(Number(e.target.value)) })}
            onPointerUp={() => void save(defaults)}
            onKeyUp={() => void save(defaults)}
          />
          <span className="mono">
            {sensitivity} <span className="muted">({defaults.thresholdDb.toFixed(1)} dB)</span>
          </span>
        </label>
        <button type="button" onClick={() => void save(null)}>
          Built-in defaults
        </button>
      </div>
      <div className="buttons">
        <button type="button" onClick={() => void clearPresets()} disabled={cleared}>
          {cleared ? "Game presets reset ✓" : "Reset all game presets"}
        </button>
      </div>
      <p className="muted">Games you tuned keep their own values until you reset them.</p>
    </>
  );
}

/** Cache sizes and one button to clear them all. */
function StorageSection({ onError }: { onError: (e: unknown) => void }) {
  const [sizes, setSizes] = useState<CacheSizes | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [clearing, setClearing] = useState(false);

  const reload = useCallback(() => unwrap(commands.getCacheSizes()).then(setSizes, onError), [onError]);

  useEffect(() => {
    void reload();
  }, [reload]);

  const clear = async () => {
    setClearing(true);
    try {
      const r = await unwrap(commands.clearCaches());
      setMessage(r.skipped > 0 ? "Cleared. Some files were in use and were kept." : "Cleared.");
      useView.getState().bumpCache();
    } catch (e) {
      onError(e);
    } finally {
      setClearing(false);
      await reload();
    }
  };

  return (
    <>
      {sizes ? (
        <p>
          Waveforms {formatBytes(sizes.waveform)} · Preview {formatBytes(sizes.preview)} · Export temp{" "}
          {formatBytes(sizes.export)}
        </p>
      ) : (
        <p className="muted">Measuring…</p>
      )}
      <div className="buttons">
        <button type="button" onClick={() => void clear()} disabled={clearing}>
          {clearing ? "Clearing…" : "Clear caches"}
        </button>
        {message && <span className="muted">{message}</span>}
      </div>
      <p className="muted">Waveforms and previews are rebuilt when a recording is opened again.</p>
    </>
  );
}
/** Preview backend and cache size. */
function PreviewSection({ onError }: { onError: (e: unknown) => void }) {
  const [settings, setSettings] = useState<PreviewSettings | null>(null);
  const [cacheMb, setCacheMb] = useState("");

  const reload = useCallback(
    () =>
      unwrap(commands.getPreviewSettings())
        .then((s) => {
          setSettings(s);
          setCacheMb(String(s.cacheMb));
        })
        .catch(onError),
    [onError],
  );

  useEffect(() => {
    void reload();
  }, [reload]);

  if (!settings) return <p className="muted">Loading…</p>;

  const save = async (backend: Backend | null, mb: number) => {
    try {
      await unwrap(commands.setPreviewSettings(backend, mb));
      await reload();
    } catch (e) {
      onError(e);
    }
  };

  const mb = Number.parseInt(cacheMb, 10);
  return (
    <>
      <div className="buttons">
        <label>
          Backend{" "}
          <select
            value={settings.backend ?? "auto"}
            onChange={(e) => void save(e.target.value === "auto" ? null : (e.target.value as Backend), settings.cacheMb)}
          >
            <option value="auto">Automatic{settings.detected ? ` (${BACKEND_NAMES[settings.detected]})` : ""}</option>
            {(Object.keys(BACKEND_NAMES) as Backend[]).map((b) => (
              <option key={b} value={b}>
                {BACKEND_NAMES[b]}
              </option>
            ))}
          </select>
        </label>
      </div>
      <p className="muted">Automatic tries the GPU first. Choosing Automatic again re-detects on the next preview.</p>
      <div className="buttons">
        <label>
          Cache limit{" "}
          <input type="number" min={100} step={100} value={cacheMb} onChange={(e) => setCacheMb(e.target.value)} /> MB
        </label>
        <button
          type="button"
          disabled={!Number.isFinite(mb) || mb === settings.cacheMb}
          onClick={() => void save(settings.backend, mb)}
        >
          Apply
        </button>
        <span className="muted">{formatBytes(settings.cacheBytes)} used</span>
      </div>
    </>
  );
}

type UpdateCheck = "idle" | "checking" | "upToDate" | "failed";

function AboutSection({ onUpdate }: { onUpdate: (update: Update) => void }) {
  const [version, setVersion] = useState<string | null>(null);
  const [supported, setSupported] = useState(false);
  const [state, setState] = useState<UpdateCheck>("idle");

  useEffect(() => {
    void getVersion().then(setVersion);
    void updaterSupported().then(setSupported);
  }, []);

  const checkNow = async () => {
    setState("checking");
    try {
      const update = await checkForUpdate();
      if (update) onUpdate(update);
      else setState("upToDate");
    } catch (e) {
      console.warn("update check failed:", e);
      setState("failed");
    }
  };

  return (
    <>
      <p>Version {version ?? "…"}</p>
      {supported ? (
        <div className="buttons">
          <button type="button" onClick={() => void checkNow()} disabled={state === "checking"}>
            {state === "checking" ? "Checking…" : "Check for updates"}
          </button>
          {state === "upToDate" && <span className="muted">Up to date.</span>}
          {state === "failed" && <span className="warn">Could not check for updates.</span>}
        </div>
      ) : (
        <p className="muted">This build does not update itself (only the Windows installer and the AppImage do).</p>
      )}
    </>
  );
}
